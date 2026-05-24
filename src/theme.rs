//! Color palette resolution.
//!
//! Three-layer precedence (matches claudebar:163-167):
//!   1. CLI overrides (passed via `Theme::with_overrides`)
//!   2. Omarchy theme at `~/.config/omarchy/current/theme/colors.toml`
//!   3. One Dark fallback
//!
//! The Omarchy file format is `key = "value"` lines (claudebar:115-131 uses a
//! regex that tolerates comments + blank lines + extra whitespace). We use
//! `toml::from_str` for the same effect — Omarchy themes are valid TOML.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// One Dark defaults, byte-identical to claudebar:152-159.
const ONE_DARK_GREEN: &str = "#98c379";
const ONE_DARK_YELLOW: &str = "#e5c07b";
const ONE_DARK_ORANGE: &str = "#d19a66";
const ONE_DARK_RED: &str = "#e06c75";
const ONE_DARK_BLUE: &str = "#61afef";
const ONE_DARK_DIM: &str = "#5c6370";
const ONE_DARK_FG: &str = "#abb2bf";
const ONE_DARK_BAR_EMPTY: &str = "#3e4451";
const ONE_DARK_MARKER: &str = "#d19a66";

/// Full resolved palette used by the widget tooltip and the TUI.
///
/// All fields are stored as `#RRGGBB` strings ready to drop into Pango
/// `<span foreground='…'>` markup. The TUI converts them to `ratatui::Color`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub green: String,
    pub yellow: String,
    pub orange: String,
    pub red: String,
    pub blue: String,
    pub dim: String,
    pub fg: String,
    pub bar_empty: String,
    pub marker: String,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            green: ONE_DARK_GREEN.into(),
            yellow: ONE_DARK_YELLOW.into(),
            orange: ONE_DARK_ORANGE.into(),
            red: ONE_DARK_RED.into(),
            blue: ONE_DARK_BLUE.into(),
            dim: ONE_DARK_DIM.into(),
            fg: ONE_DARK_FG.into(),
            bar_empty: ONE_DARK_BAR_EMPTY.into(),
            marker: ONE_DARK_MARKER.into(),
        }
    }
}

/// Subset of an Omarchy theme file we care about. Unknown keys are ignored,
/// missing keys fall back to One Dark.
#[derive(Debug, Default, Deserialize)]
struct OmarchyTheme {
    accent: Option<String>,
    foreground: Option<String>,
    background: Option<String>,
    color1: Option<String>,
    color2: Option<String>,
    color3: Option<String>,
}

impl Theme {
    /// Apply CLI `--color-*` overrides. Each `Some(_)` wins over the underlying
    /// theme; `None` preserves the resolved color.
    pub fn with_overrides(
        mut self,
        low: Option<String>,
        mid: Option<String>,
        high: Option<String>,
        critical: Option<String>,
    ) -> Self {
        if let Some(v) = low {
            self.green = v;
        }
        if let Some(v) = mid {
            self.yellow = v;
        }
        if let Some(v) = high {
            self.orange = v;
        }
        if let Some(v) = critical {
            self.red = v;
        }
        self
    }

    /// Try to load the Omarchy theme and fold it on top of `self`. Returns
    /// the result unmodified if the file is missing or unreadable — matching
    /// claudebar's silent fallback (the script never errors on missing themes).
    pub fn merged_with_omarchy(self) -> Self {
        let Some(path) = omarchy_theme_path() else {
            return self;
        };
        self.merged_with_omarchy_file(&path)
    }

    /// Same as `merged_with_omarchy` but with an explicit path (for tests).
    pub fn merged_with_omarchy_file(mut self, path: &Path) -> Self {
        let Ok(contents) = std::fs::read_to_string(path) else {
            return self;
        };
        let Ok(parsed) = toml::from_str::<OmarchyTheme>(&contents) else {
            return self;
        };

        // claudebar mapping (claudebar:133-148):
        //   accent     → blue
        //   foreground → fg
        //   color1     → red AND orange
        //   color2     → green
        //   color3     → yellow
        //   foreground+background → dim = midpoint
        //   background → bar_empty = midpoint(background, dim)
        if let Some(v) = parsed.accent {
            self.blue = v;
        }
        if let Some(v) = parsed.foreground.clone() {
            self.fg = v;
        }
        if let Some(v) = parsed.color1 {
            self.red = v.clone();
            self.orange = v;
        }
        if let Some(v) = parsed.color2 {
            self.green = v;
        }
        if let Some(v) = parsed.color3 {
            self.yellow = v;
        }
        if let (Some(fg), Some(bg)) = (&parsed.foreground, &parsed.background) {
            if let Some(dim) = hex_blend(fg, bg) {
                self.dim = dim;
                self.marker = self.dim.clone();
                if let Some(bar_empty) = hex_blend(bg, &self.dim) {
                    self.bar_empty = bar_empty;
                }
            }
        }
        self
    }
}

/// Resolve the path to the active Omarchy theme. `None` on non-Omarchy systems
/// or when `$HOME` isn't set.
fn omarchy_theme_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/omarchy/current/theme/colors.toml"))
}

/// Average two `#RRGGBB` strings into a midpoint color. Returns `None` if
/// either input isn't parseable. Mirrors claudebar's `hex_blend()`
/// (claudebar:105-110).
pub fn hex_blend(a: &str, b: &str) -> Option<String> {
    let (ar, ag, ab) = parse_hex(a)?;
    let (br, bg, bb) = parse_hex(b)?;
    Some(format!(
        "#{:02x}{:02x}{:02x}",
        (u16::from(ar) + u16::from(br)) / 2,
        (u16::from(ag) + u16::from(bg)) / 2,
        (u16::from(ab) + u16::from(bb)) / 2,
    ))
}

fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some((r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn one_dark_is_default() {
        let t = Theme::default();
        assert_eq!(t.green, ONE_DARK_GREEN);
        assert_eq!(t.red, ONE_DARK_RED);
        assert_eq!(t.bar_empty, ONE_DARK_BAR_EMPTY);
    }

    #[test]
    fn hex_blend_averages() {
        // black + white = mid grey (rounded down per integer division).
        assert_eq!(hex_blend("#000000", "#ffffff"), Some("#7f7f7f".into()));
        // red + blue = magenta-ish.
        assert_eq!(hex_blend("#ff0000", "#0000ff"), Some("#7f007f".into()));
    }

    #[test]
    fn hex_blend_rejects_garbage() {
        assert_eq!(hex_blend("not-hex", "#000000"), None);
        assert_eq!(hex_blend("#fff", "#000000"), None); // 3-digit not supported (claudebar parity)
        assert_eq!(hex_blend("#xxxxxx", "#000000"), None);
    }

    #[test]
    fn hex_blend_strips_optional_hash() {
        assert_eq!(hex_blend("000000", "ffffff"), Some("#7f7f7f".into()));
    }

    #[test]
    fn cli_overrides_win_over_defaults() {
        let t = Theme::default().with_overrides(
            Some("#111111".into()),
            None,
            Some("#222222".into()),
            None,
        );
        assert_eq!(t.green, "#111111");
        assert_eq!(t.orange, "#222222");
        assert_eq!(t.red, ONE_DARK_RED);
        assert_eq!(t.yellow, ONE_DARK_YELLOW);
    }

    #[test]
    fn missing_omarchy_file_is_silent() {
        let t = Theme::default();
        let merged = t.clone().merged_with_omarchy_file(Path::new("/nonexistent/path.toml"));
        assert_eq!(t, merged);
    }

    #[test]
    fn omarchy_overrides_palette() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            r##"
            accent = "#aabbcc"
            foreground = "#ffffff"
            background = "#000000"
            color1 = "#ff0000"
            color2 = "#00ff00"
            color3 = "#ffff00"
            "##
        )
        .unwrap();

        let t = Theme::default().merged_with_omarchy_file(f.path());
        assert_eq!(t.blue, "#aabbcc");
        assert_eq!(t.fg, "#ffffff");
        assert_eq!(t.red, "#ff0000");
        assert_eq!(t.orange, "#ff0000"); // color1 maps to both
        assert_eq!(t.green, "#00ff00");
        assert_eq!(t.yellow, "#ffff00");
        // dim = blend(fg, bg) = blend(#fff, #000) = #7f7f7f
        assert_eq!(t.dim, "#7f7f7f");
        assert_eq!(t.marker, "#7f7f7f");
        // bar_empty = blend(bg, dim) = blend(#000, #7f7f7f) = #3f3f3f
        assert_eq!(t.bar_empty, "#3f3f3f");
    }

    #[test]
    fn omarchy_partial_keys_keep_defaults_for_unset() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, r##"accent = "#123456""##).unwrap();
        let t = Theme::default().merged_with_omarchy_file(f.path());
        assert_eq!(t.blue, "#123456");
        // Untouched fields remain One Dark.
        assert_eq!(t.green, ONE_DARK_GREEN);
        assert_eq!(t.red, ONE_DARK_RED);
        assert_eq!(t.bar_empty, ONE_DARK_BAR_EMPTY);
    }

    #[test]
    fn omarchy_with_only_bg_keeps_default_dim() {
        // claudebar only computes dim when BOTH fg and bg are present.
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, r##"background = "#000000""##).unwrap();
        let t = Theme::default().merged_with_omarchy_file(f.path());
        assert_eq!(t.dim, ONE_DARK_DIM);
        assert_eq!(t.bar_empty, ONE_DARK_BAR_EMPTY);
    }

    #[test]
    fn omarchy_garbage_is_silent() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "this is not toml = = =").unwrap();
        let before = Theme::default();
        let after = before.clone().merged_with_omarchy_file(f.path());
        assert_eq!(before, after);
    }
}
