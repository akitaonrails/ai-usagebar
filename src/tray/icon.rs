//! Multi-size RGBA NotifyIcon.
//!
//! The glyph is a linear gauge mark (dial arc, needle and hub) supplied by the
//! user (Solar Icons style, CC BY 4.0). It is rasterized from
//! `windows/tray-icon.svg` into one anti-aliased `windows/tray-icon-<size>.rgba`
//! per NotifyIcon size (black ink, alpha as rendered). A dark taskbar gets the
//! same glyph in white ([`Ink::White`]); the macOS menu bar tints it itself.

/// One Dark bar colors, same thresholds the widget already uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Low,
    Mid,
    High,
    Critical,
}

impl Severity {
    pub fn from_report_str(value: &str) -> Option<Self> {
        match value {
            "low" => Some(Self::Low),
            "mid" => Some(Self::Mid),
            "high" => Some(Self::High),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }

    pub fn rank(self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Mid => 1,
            Self::High => 2,
            Self::Critical => 3,
        }
    }
}

/// NotifyIcon sizes we ship, smallest first. The shell asks for
/// `SM_CXSMICON` (16 px at 100 % DPI, 24 px at 150 %, 32 px at 200 %); handing
/// it a raster of exactly that size avoids a second resample that turned the
/// glyph jagged. Each raster is anti-aliased (alpha as rendered, RGB black).
pub const ICON_SIZES: [u32; 6] = [16, 20, 24, 32, 40, 48];

const RASTERS: [&[u8]; 6] = [
    include_bytes!("../../windows/tray-icon-16.rgba"),
    include_bytes!("../../windows/tray-icon-20.rgba"),
    include_bytes!("../../windows/tray-icon-24.rgba"),
    include_bytes!("../../windows/tray-icon-32.rgba"),
    include_bytes!("../../windows/tray-icon-40.rgba"),
    include_bytes!("../../windows/tray-icon-48.rgba"),
];

/// The shipped size that serves a request for `wanted` pixels: the exact size
/// when we have it, else the next larger one (downscaling keeps strokes),
/// else the largest.
pub fn pick_size(wanted: u32) -> u32 {
    ICON_SIZES
        .iter()
        .copied()
        .find(|size| *size >= wanted)
        .unwrap_or(ICON_SIZES[ICON_SIZES.len() - 1])
}

/// The glyph at the size `pick_size` chooses for `wanted`, independent of
/// report severity. Returns the pixels and their side length.
pub fn tray_icon_rgba(wanted: u32, _severity: Severity) -> (Vec<u8>, u32) {
    let size = pick_size(wanted);
    let index = ICON_SIZES.iter().position(|s| *s == size).unwrap_or(0);
    let bytes = RASTERS[index];
    debug_assert_eq!(bytes.len(), (size * size * 4) as usize);
    (bytes.to_vec(), size)
}

/// Stroke color for the Windows NotifyIcon: black reads on a light taskbar,
/// white on a dark one (the Windows 11 default).
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    Black,
    White,
}

/// Recolors a black-ink raster from [`tray_icon_rgba`] in place. Only RGB
/// changes; alpha, and with it the anti-aliasing, stays as rendered.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn apply_ink(rgba: &mut [u8], ink: Ink) {
    let value = match ink {
        Ink::Black => 0,
        Ink::White => 255,
    };
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel[..3].fill(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_raster_is_a_square_rgba_of_its_size() {
        for (index, size) in ICON_SIZES.iter().enumerate() {
            assert_eq!(
                RASTERS[index].len(),
                (size * size * 4) as usize,
                "size {size}"
            );
        }
    }

    #[test]
    fn pick_size_prefers_exact_then_next_larger_then_largest() {
        assert_eq!(pick_size(16), 16);
        assert_eq!(pick_size(24), 24);
        assert_eq!(pick_size(22), 24);
        assert_eq!(pick_size(36), 40);
        assert_eq!(pick_size(64), 48);
        assert_eq!(pick_size(0), 16);
    }

    #[test]
    fn severity_never_changes_the_glyph() {
        assert_eq!(
            tray_icon_rgba(24, Severity::Low),
            tray_icon_rgba(24, Severity::Critical)
        );
    }

    #[test]
    fn corners_stay_transparent_and_ink_is_black() {
        for size in ICON_SIZES {
            let (bytes, side) = tray_icon_rgba(size, Severity::Mid);
            assert_eq!(side, size);
            let last = bytes.len() - 1;
            assert_eq!(bytes[3], 0, "top-left corner at {size}");
            assert_eq!(bytes[last], 0, "bottom-right corner at {size}");
            let mut inked = 0usize;
            let mut soft = 0usize;
            for pixel in bytes.as_chunks::<4>().0 {
                if pixel[3] > 0 {
                    inked += 1;
                    assert_eq!(&pixel[..3], &[0, 0, 0], "NotifyIcon strokes are black");
                }
                if pixel[3] > 0 && pixel[3] < 255 {
                    soft += 1;
                }
            }
            assert!(
                inked > size as usize * 4,
                "glyph too sparse at {size}: {inked}"
            );
            assert!(soft > 0, "raster at {size} should be anti-aliased");
        }
    }

    #[test]
    fn white_ink_keeps_the_glyph_shape_and_changes_only_color() {
        for size in ICON_SIZES {
            let (black, _) = tray_icon_rgba(size, Severity::Mid);
            let mut white = black.clone();
            apply_ink(&mut white, Ink::White);
            for (b, w) in black
                .as_chunks::<4>()
                .0
                .iter()
                .zip(white.as_chunks::<4>().0)
            {
                assert_eq!(b[3], w[3], "alpha is untouched at {size}");
                assert_eq!(&w[..3], &[255, 255, 255], "strokes are white at {size}");
            }
            apply_ink(&mut white, Ink::Black);
            assert_eq!(white, black, "black ink restores the shipped raster");
        }
    }

    #[test]
    fn unknown_severity_strings_are_rejected() {
        assert_eq!(Severity::from_report_str("mid"), Some(Severity::Mid));
        assert_eq!(Severity::from_report_str("Hot"), None);
    }
}
