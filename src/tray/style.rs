//! The popover's look, as the page reports it on every `resize`: the host
//! switches its backdrop behind the WebView to match.

/// Width used by the native dashboard on both tray hosts, in logical points.
pub const NATIVE_WINDOW_WIDTH: f64 = 390.0;

/// `classic` draws opaque cards over the WebView's own background; `native`
/// is a translucent panel over the host OS's material (AppKit on macOS, DWM
/// Acrylic on Windows 11).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PopoverStyle {
    #[default]
    Classic,
    Native,
}

impl PopoverStyle {
    /// The exact lowercase names the page sends; anything else is no change.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "classic" => Some(Self::Classic),
            "native" => Some(Self::Native),
            _ => None,
        }
    }

    /// Width for this style, using the host's own classic width.
    pub fn window_width(self, classic: f64) -> f64 {
        match self {
            Self::Classic => classic,
            Self::Native => NATIVE_WINDOW_WIDTH,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PopoverStyle;

    #[test]
    fn parses_only_exact_style_names() {
        assert_eq!(PopoverStyle::parse("classic"), Some(PopoverStyle::Classic));
        assert_eq!(PopoverStyle::parse("native"), Some(PopoverStyle::Native));
        assert_eq!(PopoverStyle::parse("Native"), None);
        assert_eq!(PopoverStyle::parse(""), None);
        assert_eq!(PopoverStyle::parse("frosted"), None);
    }

    #[test]
    fn defaults_to_classic() {
        assert_eq!(PopoverStyle::default(), PopoverStyle::Classic);
    }

    #[test]
    fn uses_host_classic_width_and_shared_native_width() {
        assert_eq!(PopoverStyle::Classic.window_width(320.0), 320.0);
        assert_eq!(PopoverStyle::Classic.window_width(300.0), 300.0);
        assert_eq!(PopoverStyle::Native.window_width(320.0), 390.0);
        assert_eq!(PopoverStyle::Native.window_width(300.0), 390.0);
    }
}
