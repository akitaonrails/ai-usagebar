//! Color palette resolution.
//!
//! Three-layer precedence (matches claudebar:163-167):
//!   1. CLI overrides (passed via `Theme::with_overrides`)
//!   2. The active Omarchy theme's `colors.toml`
//!   3. One Dark fallback
//!
//! Omarchy applies a theme into `~/.local/state/omarchy/current/theme`; the
//! `~/.config/omarchy/current/theme` location this port originally read is the
//! older layout and stays as a fallback. The Omarchy file format is
//! `key = "value"` lines (claudebar:115-131 uses a regex that tolerates
//! comments + blank lines + extra whitespace). We use `toml::from_str` for the
//! same effect - Omarchy themes are valid TOML.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

const MAX_OMARCHY_FILE: u64 = 64 * 1024;
const OMARCHY_COLORS: &[&str] = &[
    "background",
    "dark_background",
    "darker_background",
    "lighter_background",
    "foreground",
    "dark_foreground",
    "light_foreground",
    "bright_foreground",
    "accent",
    "muted",
    "selection",
    "red",
    "yellow",
    "orange",
    "green",
    "blue",
    "cyan",
    "magenta",
];

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
/// missing keys fall back to One Dark. Themes name these colors as of Omarchy
/// 4 (`red`/`green`/`yellow`); `color1`-`color3` are the older aliases and
/// still resolve, so a theme carrying either layout works.
#[derive(Debug, Default, Deserialize)]
struct OmarchyTheme {
    accent: Option<String>,
    foreground: Option<String>,
    background: Option<String>,
    red: Option<String>,
    green: Option<String>,
    yellow: Option<String>,
    color1: Option<String>,
    color2: Option<String>,
    color3: Option<String>,
}

/// The active theme's palette and corner radius for the Linux tray popover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OmarchyAppearance {
    pub mode: &'static str,
    pub rounding: u32,
    pub colors: BTreeMap<&'static str, String>,
}

impl OmarchyAppearance {
    /// Load the palette from a theme directory. The compositor supplies rounding.
    pub fn read_from(theme_dir: &Path) -> Option<Self> {
        let palette: toml::Table = read_omarchy_toml(&theme_dir.join("colors.toml"))?;
        let mode = match palette.get("mode").and_then(toml::Value::as_str) {
            Some("light") => "light",
            _ => "dark",
        };
        let mut colors: BTreeMap<_, _> = OMARCHY_COLORS
            .iter()
            .filter_map(|&key| {
                let value = palette.get(key)?.as_str()?;
                (value.starts_with('#') && parse_hex_rgb(value).is_some())
                    .then(|| (key, value.to_owned()))
            })
            .collect();
        if !colors.contains_key("accent")
            && let Some(blue) = colors.get("blue")
        {
            colors.insert("accent", blue.clone());
        }
        Some(Self {
            mode,
            rounding: 0,
            colors,
        })
    }

    #[cfg(all(target_os = "linux", feature = "linux-tray"))]
    pub(crate) fn active_in(home: &Path) -> Option<Self> {
        let palette = omarchy_theme_path_in(home)?;
        Self::read_from(palette.parent()?)
    }
}

fn read_omarchy_toml<T: DeserializeOwned>(path: &Path) -> Option<T> {
    toml::from_str(&read_omarchy_text(path)?).ok()
}

pub(crate) fn read_omarchy_text(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    // Even if the path is replaced between the checks, opening a FIFO must
    // never stall the watcher. Symlinks to regular theme files remain usable.
    #[cfg(unix)]
    let file = {
        use rustix::fs::{Mode, OFlags, open};
        File::from(
            open(
                path,
                OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .ok()?,
        )
    };
    #[cfg(not(unix))]
    let file = File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut contents = String::new();
    file.take(MAX_OMARCHY_FILE + 1)
        .read_to_string(&mut contents)
        .ok()?;
    (contents.len() as u64 <= MAX_OMARCHY_FILE).then_some(contents)
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
        let Some(parsed) = read_omarchy_toml::<OmarchyTheme>(path) else {
            return self;
        };

        // claudebar mapping (claudebar:133-148), with the named keys Omarchy
        // writes today winning over the older color1-3 aliases:
        //   accent       → blue
        //   foreground   → fg
        //   red/color1   → red AND orange
        //   green/color2 → green
        //   yellow/color3 → yellow
        //   foreground+background → dim = midpoint
        //   background → bar_empty = midpoint(background, dim)
        if let Some(v) = parsed.accent {
            self.blue = v;
        }
        if let Some(v) = parsed.foreground.clone() {
            self.fg = v;
        }
        if let Some(v) = parsed.red.or(parsed.color1) {
            self.red = v.clone();
            self.orange = v;
        }
        if let Some(v) = parsed.green.or(parsed.color2) {
            self.green = v;
        }
        if let Some(v) = parsed.yellow.or(parsed.color3) {
            self.yellow = v;
        }
        if let (Some(fg), Some(bg)) = (&parsed.foreground, &parsed.background)
            && let Some(dim) = hex_blend(fg, bg)
        {
            self.dim = dim;
            self.marker = self.dim.clone();
            if let Some(bar_empty) = hex_blend(bg, &self.dim) {
                self.bar_empty = bar_empty;
            }
        }
        self
    }
}

/// Resolve the path to the active Omarchy theme. `None` on non-Omarchy systems
/// or when `$HOME` isn't set.
fn omarchy_theme_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    omarchy_theme_path_in(Path::new(&home))
}

/// Same as `omarchy_theme_path` but with an explicit home (for tests).
fn omarchy_theme_path_in(home: &Path) -> Option<PathBuf> {
    [
        // Where `omarchy-theme-set` applies the active theme.
        home.join(".local/state/omarchy/current/theme/colors.toml"),
        // The layout this port and claudebar originally read.
        home.join(".config/omarchy/current/theme/colors.toml"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

/// Average two `#RRGGBB` strings into a midpoint color. Returns `None` if
/// either input isn't parseable. Mirrors claudebar's `hex_blend()`
/// (claudebar:105-110).
pub fn hex_blend(a: &str, b: &str) -> Option<String> {
    let (ar, ag, ab) = parse_hex_rgb(a)?;
    let (br, bg, bb) = parse_hex_rgb(b)?;
    Some(format!(
        "#{:02x}{:02x}{:02x}",
        (u16::from(ar) + u16::from(br)) / 2,
        (u16::from(ag) + u16::from(bg)) / 2,
        (u16::from(ab) + u16::from(bb)) / 2,
    ))
}

pub(crate) fn parse_hex_rgb(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.strip_prefix('#').unwrap_or(s);
    let [r1, r2, g1, g2, b1, b2] = s.as_bytes() else {
        return None;
    };
    Some((
        hex_pair(*r1, *r2)?,
        hex_pair(*g1, *g2)?,
        hex_pair(*b1, *b2)?,
    ))
}

fn hex_pair(hi: u8, lo: u8) -> Option<u8> {
    Some((hex_nibble(hi)? << 4) | hex_nibble(lo)?)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
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
        assert_eq!(hex_blend("aéabc", "#000000"), None);
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
        let merged = t
            .clone()
            .merged_with_omarchy_file(Path::new("/nonexistent/path.toml"));
        assert_eq!(t, merged);
    }

    #[test]
    fn omarchy_named_keys_override_palette() {
        // Omarchy 4 themes name their colors. Same slots as the color1-3
        // aliases above, so a theme may carry either layout.
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            r##"
            accent = "#aabbcc"
            foreground = "#ffffff"
            background = "#000000"
            red = "#ff0000"
            green = "#00ff00"
            yellow = "#ffff00"
            "##
        )
        .unwrap();

        let t = Theme::default().merged_with_omarchy_file(f.path());
        assert_eq!(t.blue, "#aabbcc");
        assert_eq!(t.fg, "#ffffff");
        assert_eq!(t.red, "#ff0000");
        assert_eq!(t.orange, "#ff0000"); // red maps to both
        assert_eq!(t.green, "#00ff00");
        assert_eq!(t.yellow, "#ffff00");
    }

    #[test]
    fn omarchy_named_keys_win_over_aliases() {
        // A theme that kept the old aliases next to the named keys keeps the
        // named value, which is what Omarchy itself reads.
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            r##"
            red = "#111111"
            green = "#222222"
            yellow = "#333333"
            color1 = "#aaaaaa"
            color2 = "#bbbbbb"
            color3 = "#cccccc"
            "##
        )
        .unwrap();

        let t = Theme::default().merged_with_omarchy_file(f.path());
        assert_eq!(t.red, "#111111");
        assert_eq!(t.orange, "#111111");
        assert_eq!(t.green, "#222222");
        assert_eq!(t.yellow, "#333333");
    }

    #[test]
    fn omarchy_theme_path_follows_where_the_theme_is_applied() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(omarchy_theme_path_in(home.path()), None);

        // The pre-Omarchy-4 layout is still read, and only while the applied
        // theme is missing.
        let legacy = home
            .path()
            .join(".config/omarchy/current/theme/colors.toml");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, "accent = \"#123456\"\n").unwrap();
        assert_eq!(omarchy_theme_path_in(home.path()), Some(legacy.clone()));

        let applied = home
            .path()
            .join(".local/state/omarchy/current/theme/colors.toml");
        std::fs::create_dir_all(applied.parent().unwrap()).unwrap();
        std::fs::write(&applied, "accent = \"#654321\"\n").unwrap();
        assert_eq!(omarchy_theme_path_in(home.path()), Some(applied));
    }

    #[test]
    fn omarchy_appearance_reads_the_complete_palette() {
        let theme = tempfile::tempdir().unwrap();
        let palette = [
            ("background", "#1a1b26"),
            ("dark_background", "#13141c"),
            ("darker_background", "#0e0e14"),
            ("lighter_background", "#24283b"),
            ("foreground", "#a9b1d6"),
            ("dark_foreground", "#565f89"),
            ("light_foreground", "#b4bee6"),
            ("bright_foreground", "#c0caf5"),
            ("accent", "#7aa2f7"),
            ("muted", "#414868"),
            ("selection", "#292e42"),
            ("red", "#f7768e"),
            ("yellow", "#e0af68"),
            ("orange", "#eb927b"),
            ("green", "#9ece6a"),
            ("blue", "#7aa2f7"),
            ("cyan", "#449dab"),
            ("magenta", "#ad8ee6"),
        ];
        let mut colors = String::from("mode = \"light\"\nbrown = \"#75493d\"\n");
        for (key, value) in palette {
            colors.push_str(&format!("{key} = \"{value}\"\n"));
        }
        std::fs::write(theme.path().join("colors.toml"), colors).unwrap();
        let appearance = OmarchyAppearance::read_from(theme.path()).unwrap();
        assert_eq!(appearance.mode, "light");
        assert_eq!(appearance.colors.len(), palette.len());
        for (key, value) in palette {
            assert_eq!(appearance.colors[key], value, "{key}");
        }
    }

    #[test]
    fn omarchy_appearance_omits_invalid_colors_and_falls_back_to_blue() {
        let theme = tempfile::tempdir().unwrap();
        let colors = theme.path().join("colors.toml");
        assert!(OmarchyAppearance::read_from(theme.path()).is_none());
        std::fs::write(
            &colors,
            r##"
            background = "#abc"
            dark_background = "123456"
            red = "#abcdef00"
            green = "#12345z"
            orange = 42
            foreground = "#123456 "
            blue = "#ABCDEF"
            "##,
        )
        .unwrap();
        let appearance = OmarchyAppearance::read_from(theme.path()).unwrap();
        assert_eq!(appearance.mode, "dark");
        assert_eq!(appearance.colors.len(), 2);
        assert_eq!(appearance.colors["blue"], "#ABCDEF");
        assert_eq!(appearance.colors["accent"], "#ABCDEF");

        // Even a valid prefix cannot make an oversized file a usable theme.
        std::fs::write(&colors, format!("mode = \"light\"\n#{}", " ".repeat(65536))).unwrap();
        assert!(OmarchyAppearance::read_from(theme.path()).is_none());
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
