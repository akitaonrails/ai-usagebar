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

    /// Width in logical points for a `[tray] width` setting. `None` is this style's own
    /// width, returned untouched so an unset key sizes the popover exactly as before the key
    /// existed. A set width is held to [`width_limits`] and then to `available`, the screen
    /// width the host measured, less [`SCREEN_INSET`] on each side. `available` is `None`
    /// when the host could not find the screen, which leaves the width unconstrained by it.
    pub fn configured_width(self, classic: f64, width: Option<u16>, available: Option<f64>) -> f64 {
        let original = self.window_width(classic);
        let Some(width) = width else {
            return original;
        };
        let (minimum, maximum) = width_limits(original);
        let bounded = f64::from(width.clamp(minimum, maximum));
        match available {
            Some(available) if available.is_finite() => {
                bounded.min((available - 2.0 * SCREEN_INSET).max(1.0))
            }
            _ => bounded,
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

/// Gap kept on each side of a configured width on a screen narrower than it asks for.
const SCREEN_INSET: f64 = 16.0;

/// The narrowest and widest a style may be set, in logical points: three quarters and three
/// times `original`, that style's own width. Three quarters is where every Settings row still
/// shows a readable label next to its control; at half, the labels shrank to a letter or two.
/// `original` is the host's constant, never IPC input.
pub(crate) fn width_limits(original: f64) -> (u16, u16) {
    (
        (original * 0.75).ceil() as u16,
        (original * 3.0).floor() as u16,
    )
}

/// The width a `set-popover-width {width}` asks for: `null` is no width (the style's own),
/// and anything else must be an integer inside [`width_limits`]. The refusal names the limits.
#[cfg(any(windows, target_os = "macos", test))]
fn requested_width(value: &serde_json::Value, original: f64) -> Result<Option<u16>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let (minimum, maximum) = width_limits(original);
    value
        .as_u64()
        .and_then(|width| u16::try_from(width).ok())
        .filter(|width| (minimum..=maximum).contains(width))
        .map(Some)
        .ok_or_else(|| {
            format!(
                "Popover width must be a whole number from {minimum} to {maximum}, got {value}."
            )
        })
}

/// `set-popover-width {width}`: check the page's value with [`requested_width`] and write
/// `[tray] width` before a host resizes anything; `null` removes the key. The error is the
/// reason nothing was written, worded for the line under the control: the limits, a config
/// with no location, or the config writer's own error (a lock held by another process, an
/// I/O failure, a file that does not parse). The writer names a parse error's line and column
/// without quoting the line (`config::toml_error_summary`), so none of them can carry a key.
/// The path is a parameter so tests never touch a real config.
#[cfg(any(windows, target_os = "macos", test))]
pub(crate) fn save_width(
    path: Option<&std::path::Path>,
    value: &serde_json::Value,
    original: f64,
) -> Result<Option<u16>, String> {
    let width = requested_width(value, original)?;
    let path = path.ok_or("config.toml has no location here, so the width was not saved.")?;
    crate::config::set_tray_value(path, "width", width.map(|width| i64::from(width).into()))
        .map_err(|error| error.to_string())?;
    Ok(width)
}

#[cfg(test)]
mod tests {
    use super::PopoverStyle;

    /// The claim the setting rests on: with `[tray] width` unset the popover is sized as it
    /// was before the key existed, whatever the screen.
    #[test]
    fn an_unset_width_is_the_style_width_on_any_screen() {
        for available in [Some(1920.0), Some(200.0), Some(f64::NAN), None] {
            for classic in [300.0, 320.0] {
                assert_eq!(
                    PopoverStyle::Classic.configured_width(classic, None, available),
                    PopoverStyle::Classic.window_width(classic)
                );
                assert_eq!(
                    PopoverStyle::Native.configured_width(classic, None, available),
                    super::NATIVE_WINDOW_WIDTH
                );
            }
        }
    }

    #[test]
    fn a_set_width_is_held_to_the_style_limits_then_to_the_screen() {
        let wide = Some(4000.0);
        let classic = |width, available| {
            PopoverStyle::Classic.configured_width(300.0, Some(width), available)
        };
        assert_eq!(classic(350, wide), 350.0);
        // A hand-edited value outside the limits is pulled back to them.
        assert_eq!(classic(0, wide), 225.0);
        assert_eq!(classic(1, wide), 225.0);
        assert_eq!(classic(u16::MAX, wide), 900.0);
        // The limits follow the style, so the same value reads differently in Native.
        let native =
            |width, available| PopoverStyle::Native.configured_width(300.0, Some(width), available);
        assert_eq!(native(1, wide), 293.0);
        assert_eq!(native(u16::MAX, wide), 1170.0);
        // A narrower screen wins, keeping the inset on each side.
        assert_eq!(native(800, Some(640.0)), 608.0);
        assert_eq!(native(800, Some(20.0)), 1.0);
        // No screen found: the limits alone apply.
        assert_eq!(native(800, None), 800.0);
        assert_eq!(native(800, Some(f64::NAN)), 800.0);
    }

    #[test]
    fn save_width_accepts_the_limits_and_refuses_one_point_past_them() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        for (original, minimum, maximum) in
            [(300.0, 225, 900), (320.0, 240, 960), (390.0, 293, 1170)]
        {
            assert_eq!(super::width_limits(original), (minimum, maximum));
            for width in [minimum, maximum] {
                assert_eq!(
                    super::save_width(Some(&path), &serde_json::json!(width), original).unwrap(),
                    Some(width)
                );
            }
            let written = std::fs::read(&path).unwrap();
            for width in [minimum - 1, maximum + 1] {
                assert!(
                    super::save_width(Some(&path), &serde_json::json!(width), original).is_err()
                );
                assert_eq!(std::fs::read(&path).unwrap(), written);
            }
        }
    }

    #[test]
    fn save_width_keeps_the_rest_of_the_file_and_null_removes_the_key() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let original =
            "# operator note\n[tray]\nrefresh_minutes = 10 # keep\n[ui]\nprimary = \"openai\"\n";
        std::fs::write(&path, original).unwrap();
        assert_eq!(
            super::save_width(Some(&path), &serde_json::json!(337), 300.0).unwrap(),
            Some(337)
        );
        assert_eq!(
            crate::config::Config::load_from(&path).unwrap().tray.width,
            Some(337)
        );
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("# operator note"));
        assert!(saved.contains("refresh_minutes = 10 # keep"));
        assert!(saved.contains("primary = \"openai\""));
        assert_eq!(
            super::save_width(Some(&path), &serde_json::Value::Null, 300.0).unwrap(),
            None
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn a_refused_width_leaves_the_file_alone() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let original = "[tray]\nwidth = 350\n";
        std::fs::write(&path, original).unwrap();
        for value in [
            serde_json::json!(-1),
            serde_json::json!(350.5),
            serde_json::json!(70000),
            serde_json::json!("350"),
            serde_json::json!(true),
            serde_json::json!({}),
        ] {
            let refused = super::save_width(Some(&path), &value, 300.0).unwrap_err();
            assert!(refused.contains("from 225 to 900"), "{refused}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }
    }

    /// The line under the control says which of the failures it was, and a config that does
    /// not parse is named by position only: the line it stopped at can hold a key.
    #[test]
    fn a_width_that_was_not_saved_says_why() {
        let directory = tempfile::tempdir().unwrap();
        let width = serde_json::json!(350);
        let refused = super::save_width(None, &serde_json::json!(901), 300.0).unwrap_err();
        assert!(refused.contains("from 225 to 900, got 901"), "{refused}");
        let nowhere = super::save_width(None, &width, 300.0).unwrap_err();
        assert!(nowhere.contains("no location"), "{nowhere}");

        let broken = directory.path().join("config.toml");
        std::fs::write(&broken, "[zai]\napi_key = zk-unquoted\n").unwrap();
        let unparsed = super::save_width(Some(&broken), &width, 300.0).unwrap_err();
        assert!(unparsed.contains("line 2,"), "{unparsed}");
        assert!(!unparsed.contains("zk-unquoted"), "{unparsed}");
        assert_eq!(
            std::fs::read_to_string(&broken).unwrap(),
            "[zai]\napi_key = zk-unquoted\n"
        );

        // A path that cannot be written is an I/O error, and a different sentence again.
        let unwritable = directory.path().join("directory");
        std::fs::create_dir(&unwritable).unwrap();
        let failed = super::save_width(Some(&unwritable), &width, 300.0).unwrap_err();
        for other in [&refused, &nowhere, &unparsed] {
            assert_ne!(&failed, other);
        }
    }

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
