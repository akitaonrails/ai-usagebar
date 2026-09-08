//! Global toggle shortcut for the tray popover.
//!
//! [`normalize`] is pure and compiles everywhere so Linux CI can test it: it
//! turns whatever the user typed ("shift + ctrl + u") into the one canonical
//! spelling the Settings page echoes back ("Ctrl+Shift+U") and the dialect
//! `global_hotkey::hotkey::HotKey::from_str` parses ("control+shift+KeyU").
//! [`HotkeyBinding`] is the Windows-only registration on top of it.

use std::fmt::Write as _;

/// Canonical spelling shown to the user ("Ctrl+Shift+U") plus the dialect
/// `global_hotkey::hotkey::HotKey::from_str` parses ("control+shift+KeyU").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Normalized {
    pub canonical: String,
    pub crate_form: String,
}

/// Longest slice of an unrecognised token echoed back in an error. The text
/// came from a config file or a text box, so it is data, not a message.
const MAX_ECHOED_TOKEN_CHARS: usize = 16;

/// Parse a user-typed shortcut into its canonical and crate spellings.
///
/// Tokens are split on `+`, trimmed, and matched case-insensitively. At least
/// one of Ctrl/Alt/Win is required (Shift alone would shadow typing) and
/// exactly one non-modifier key. Escape is refused because it already closes
/// the popover. Errors are user-facing sentences.
pub fn normalize(text: &str) -> Result<Normalized, String> {
    if text.trim().is_empty() {
        return Err("Enter a shortcut".to_string());
    }

    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut win = false;
    let mut key: Option<Key> = None;

    for raw in text.split('+') {
        let token = raw.trim();
        if token.is_empty() {
            return Err("Put a key between each '+'".to_string());
        }
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" | "option" => alt = true,
            "shift" => shift = true,
            "win" | "super" | "meta" | "cmd" | "command" => win = true,
            _ => {
                if key.is_some() {
                    return Err("Use exactly one key, for example Ctrl+Shift+U".to_string());
                }
                key = Some(parse_key(token)?);
            }
        }
    }

    let Some(key) = key else {
        return Err("Add a key, for example Ctrl+Shift+U".to_string());
    };
    if !(ctrl || alt || win) {
        return Err("Add Ctrl, Alt or Win".to_string());
    }

    let mut canonical = String::new();
    let mut crate_form = String::new();
    for (on, shown, parsed) in [
        (ctrl, "Ctrl", "control"),
        (alt, "Alt", "alt"),
        (shift, "Shift", "shift"),
        (win, "Win", "super"),
    ] {
        if on {
            let _ = write!(canonical, "{shown}+");
            let _ = write!(crate_form, "{parsed}+");
        }
    }
    canonical.push_str(&key.canonical);
    crate_form.push_str(&key.crate_form);

    Ok(Normalized {
        canonical,
        crate_form,
    })
}

struct Key {
    canonical: String,
    crate_form: String,
}

impl Key {
    fn fixed(canonical: &str, crate_form: &str) -> Self {
        Self {
            canonical: canonical.to_string(),
            crate_form: crate_form.to_string(),
        }
    }
}

fn parse_key(token: &str) -> Result<Key, String> {
    let mut chars = token.chars();
    if let (Some(ch), None) = (chars.next(), chars.next()) {
        if ch.is_ascii_alphabetic() {
            let upper = ch.to_ascii_uppercase();
            return Ok(Key {
                canonical: upper.to_string(),
                crate_form: format!("Key{upper}"),
            });
        }
        if ch.is_ascii_digit() {
            return Ok(Key {
                canonical: ch.to_string(),
                crate_form: format!("Digit{ch}"),
            });
        }
        let punctuation = match ch {
            '-' => Some("Minus"),
            '=' => Some("Equal"),
            '[' => Some("BracketLeft"),
            ']' => Some("BracketRight"),
            '\\' => Some("Backslash"),
            ';' => Some("Semicolon"),
            '\'' => Some("Quote"),
            ',' => Some("Comma"),
            '.' => Some("Period"),
            '/' => Some("Slash"),
            '`' => Some("Backquote"),
            _ => None,
        };
        if let Some(crate_form) = punctuation {
            return Ok(Key::fixed(&ch.to_string(), crate_form));
        }
    }

    let lower = token.to_ascii_lowercase();
    if let Some(number) = lower.strip_prefix('f')
        && number.bytes().all(|b| b.is_ascii_digit())
        && let Ok(n) = number.parse::<u8>()
        && (1..=24).contains(&n)
    {
        let name = format!("F{n}");
        return Ok(Key::fixed(&name, &name));
    }

    let named = match lower.as_str() {
        "space" => Some(("Space", "Space")),
        "enter" | "return" => Some(("Enter", "Enter")),
        "tab" => Some(("Tab", "Tab")),
        "backspace" => Some(("Backspace", "Backspace")),
        "delete" | "del" => Some(("Delete", "Delete")),
        "insert" | "ins" => Some(("Insert", "Insert")),
        "home" => Some(("Home", "Home")),
        "end" => Some(("End", "End")),
        "pageup" | "pgup" => Some(("PageUp", "PageUp")),
        "pagedown" | "pgdn" => Some(("PageDown", "PageDown")),
        "up" | "arrowup" => Some(("Up", "ArrowUp")),
        "down" | "arrowdown" => Some(("Down", "ArrowDown")),
        "left" | "arrowleft" => Some(("Left", "ArrowLeft")),
        "right" | "arrowright" => Some(("Right", "ArrowRight")),
        "escape" | "esc" => return Err("Escape closes the popover".to_string()),
        _ => None,
    };
    if let Some((canonical, crate_form)) = named {
        return Ok(Key::fixed(canonical, crate_form));
    }

    Err(format!("Unsupported key: {}", echo_token(token)))
}

/// Bound and ASCII-only: the token is echoed into a sentence a UI renders.
fn echo_token(token: &str) -> String {
    let echoed: String = token
        .chars()
        .filter(char::is_ascii_graphic)
        .take(MAX_ECHOED_TOKEN_CHARS)
        .collect();
    if echoed.is_empty() {
        "?".to_string()
    } else {
        echoed
    }
}

#[cfg(windows)]
mod binding {
    use global_hotkey::hotkey::HotKey;
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

    use crate::display::sanitize_untrusted_line;

    use super::normalize;

    /// The one registered global shortcut, or none.
    ///
    /// The manager owns a message-only window that receives `WM_HOTKEY`, so
    /// the binding lives on the thread that pumps messages: the tao
    /// event-loop thread, the same one the tray icon lives on.
    pub struct HotkeyBinding {
        manager: GlobalHotKeyManager,
        current: Option<HotKey>,
    }

    impl HotkeyBinding {
        /// Must be created on the thread that runs the win32 message loop
        /// (the tao event-loop thread).
        pub fn new() -> Result<Self, String> {
            let manager = GlobalHotKeyManager::new().map_err(|err| {
                format!(
                    "Could not set up the global shortcut: {}",
                    sanitize_untrusted_line(&err.to_string())
                )
            })?;
            Ok(Self {
                manager,
                current: None,
            })
        }

        /// Replace the registered shortcut. `None` unregisters. On failure
        /// nothing stays registered and the error is a user-facing sentence
        /// ("Ctrl+Shift+U is already used by another app").
        pub fn apply(&mut self, canonical: Option<&str>) -> Result<(), String> {
            let wanted = match canonical {
                None => None,
                Some(text) => {
                    let normalized = normalize(text)?;
                    let hotkey: HotKey = normalized.crate_form.parse().map_err(|err| {
                        format!(
                            "{} is not a shortcut this build can register: {}",
                            normalized.canonical,
                            sanitize_untrusted_line(&format!("{err}"))
                        )
                    })?;
                    Some((normalized.canonical, hotkey))
                }
            };

            if let (Some(current), Some((_, hotkey))) = (self.current, &wanted)
                && current == *hotkey
            {
                return Ok(());
            }

            if let Some(previous) = self.current.take() {
                self.manager.unregister(previous).map_err(|err| {
                    format!(
                        "Could not release the previous shortcut: {}",
                        sanitize_untrusted_line(&err.to_string())
                    )
                })?;
            }

            let Some((canonical, hotkey)) = wanted else {
                return Ok(());
            };
            self.manager
                .register(hotkey)
                .map_err(|err| register_failure(&canonical, &err))?;
            self.current = Some(hotkey);
            Ok(())
        }

        /// Id the crate reports in press events for the registered shortcut.
        pub fn current_id(&self) -> Option<u32> {
            self.current.map(|hotkey| hotkey.id())
        }
    }

    /// Turn a registration failure into the sentence the Settings page shows.
    fn register_failure(canonical: &str, err: &global_hotkey::Error) -> String {
        match err {
            global_hotkey::Error::AlreadyRegistered(_) => {
                format!("{canonical} is already used by another app")
            }
            other => format!(
                "Could not register {canonical}: {}",
                sanitize_untrusted_line(&other.to_string())
            ),
        }
    }

    /// Route presses (`HotKeyState::Pressed` only) to the caller; replaces the
    /// crate's channel receiver. The crate keeps the first handler installed
    /// for the life of the process, so call this once.
    pub fn install_press_handler<F: Fn(u32) + Send + Sync + 'static>(handler: F) {
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state() == HotKeyState::Pressed {
                handler(event.id());
            }
        }));
    }

    #[cfg(test)]
    mod tests {
        use std::str::FromStr;

        use global_hotkey::hotkey::{Code, HotKey, Modifiers};

        use super::super::normalize;
        use super::register_failure;

        #[test]
        fn crate_parses_the_crate_form_we_emit() {
            let normalized = normalize("Ctrl+Shift+U").expect("valid shortcut");
            let hotkey = HotKey::from_str(&normalized.crate_form).expect("crate accepts it");
            assert_eq!(hotkey.mods, Modifiers::CONTROL | Modifiers::SHIFT);
            assert_eq!(hotkey.key, Code::KeyU);
        }

        #[test]
        fn crate_parses_every_key_family_we_emit() {
            for text in [
                "Win+Alt+F12",
                "Ctrl+1",
                "Alt+Up",
                "Ctrl+Alt+Space",
                "Ctrl+`",
                "Ctrl+Shift+\\",
            ] {
                let normalized = normalize(text).expect(text);
                HotKey::from_str(&normalized.crate_form).expect(text);
            }
        }

        #[test]
        fn already_registered_becomes_a_friendly_sentence() {
            let hotkey = HotKey::new(Some(Modifiers::CONTROL), Code::KeyU);
            let message =
                register_failure("Ctrl+U", &global_hotkey::Error::AlreadyRegistered(hotkey));
            assert_eq!(message, "Ctrl+U is already used by another app");
        }

        #[test]
        fn other_failures_name_the_shortcut_and_strip_controls() {
            let err = global_hotkey::Error::FailedToRegister("bad\u{1b}[31m vk".to_string());
            let message = register_failure("Ctrl+U", &err);
            assert!(
                message.starts_with("Could not register Ctrl+U: "),
                "{message}"
            );
            assert!(!message.contains('\u{1b}'), "{message}");
        }
    }
}

#[cfg(windows)]
pub use binding::{HotkeyBinding, install_press_handler};

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical(text: &str) -> String {
        normalize(text)
            .unwrap_or_else(|err| panic!("{text}: {err}"))
            .canonical
    }

    fn crate_form(text: &str) -> String {
        normalize(text)
            .unwrap_or_else(|err| panic!("{text}: {err}"))
            .crate_form
    }

    fn refusal(text: &str) -> String {
        match normalize(text) {
            Ok(normalized) => panic!("{text} was accepted as {normalized:?}"),
            Err(err) => err,
        }
    }

    #[test]
    fn modifiers_come_out_in_canonical_order() {
        assert_eq!(canonical("shift+ctrl+u"), "Ctrl+Shift+U");
        assert_eq!(canonical("win+shift+alt+ctrl+k"), "Ctrl+Alt+Shift+Win+K");
    }

    #[test]
    fn whitespace_and_case_are_forgiven() {
        assert_eq!(canonical("  CTRL +  Shift + u "), "Ctrl+Shift+U");
    }

    #[test]
    fn modifier_aliases_collapse() {
        assert_eq!(canonical("control+u"), "Ctrl+U");
        assert_eq!(canonical("option+u"), "Alt+U");
        for win in ["win", "super", "meta", "cmd", "command"] {
            assert_eq!(canonical(&format!("{win}+u")), "Win+U", "{win}");
        }
    }

    #[test]
    fn repeated_modifiers_are_harmless() {
        assert_eq!(canonical("ctrl+control+u"), "Ctrl+U");
    }

    #[test]
    fn function_keys() {
        assert_eq!(canonical("alt+f1"), "Alt+F1");
        assert_eq!(canonical("ctrl+F24"), "Ctrl+F24");
        assert_eq!(crate_form("ctrl+F24"), "control+F24");
    }

    #[test]
    fn arrows_use_short_canonical_and_crate_names() {
        assert_eq!(canonical("ctrl+arrowup"), "Ctrl+Up");
        assert_eq!(crate_form("ctrl+up"), "control+ArrowUp");
        assert_eq!(crate_form("alt+left"), "alt+ArrowLeft");
    }

    #[test]
    fn punctuation_keys() {
        assert_eq!(canonical("ctrl+-"), "Ctrl+-");
        assert_eq!(crate_form("ctrl+-"), "control+Minus");
        assert_eq!(crate_form("ctrl+="), "control+Equal");
        assert_eq!(crate_form("ctrl+["), "control+BracketLeft");
        assert_eq!(crate_form("ctrl+]"), "control+BracketRight");
        assert_eq!(crate_form("ctrl+\\"), "control+Backslash");
        assert_eq!(crate_form("ctrl+;"), "control+Semicolon");
        assert_eq!(crate_form("ctrl+'"), "control+Quote");
        assert_eq!(crate_form("ctrl+,"), "control+Comma");
        assert_eq!(crate_form("ctrl+."), "control+Period");
        assert_eq!(crate_form("ctrl+/"), "control+Slash");
        assert_eq!(crate_form("ctrl+`"), "control+Backquote");
    }

    #[test]
    fn digits_and_named_keys() {
        assert_eq!(canonical("ctrl+1"), "Ctrl+1");
        assert_eq!(crate_form("ctrl+1"), "control+Digit1");
        assert_eq!(canonical("ctrl+space"), "Ctrl+Space");
        assert_eq!(canonical("ctrl+pgup"), "Ctrl+PageUp");
        assert_eq!(crate_form("win+enter"), "super+Enter");
    }

    #[test]
    fn crate_form_spells_letters_as_key_codes() {
        assert_eq!(crate_form("Ctrl+Shift+U"), "control+shift+KeyU");
        assert_eq!(crate_form("win+alt+z"), "alt+super+KeyZ");
    }

    #[test]
    fn shift_alone_is_not_enough() {
        assert_eq!(refusal("Shift+U"), "Add Ctrl, Alt or Win");
    }

    #[test]
    fn a_bare_key_is_refused() {
        assert_eq!(refusal("u"), "Add Ctrl, Alt or Win");
    }

    #[test]
    fn two_keys_are_refused() {
        assert_eq!(
            refusal("ctrl+u+i"),
            "Use exactly one key, for example Ctrl+Shift+U"
        );
    }

    #[test]
    fn only_modifiers_are_refused() {
        assert_eq!(refusal("ctrl+shift"), "Add a key, for example Ctrl+Shift+U");
    }

    #[test]
    fn escape_is_refused() {
        assert_eq!(refusal("ctrl+escape"), "Escape closes the popover");
        assert_eq!(refusal("ctrl+esc"), "Escape closes the popover");
    }

    #[test]
    fn blank_is_refused() {
        assert_eq!(refusal(""), "Enter a shortcut");
        assert_eq!(refusal("   "), "Enter a shortcut");
    }

    #[test]
    fn an_empty_token_is_refused() {
        assert_eq!(refusal("ctrl++u"), "Put a key between each '+'");
    }

    #[test]
    fn unknown_tokens_are_echoed() {
        assert_eq!(refusal("ctrl+numpad5"), "Unsupported key: numpad5");
        assert_eq!(refusal("ctrl+f25"), "Unsupported key: f25");
        assert_eq!(refusal("ctrl+f0"), "Unsupported key: f0");
        assert_eq!(refusal("ctrl+f1x"), "Unsupported key: f1x");
    }

    #[test]
    fn overlong_garbage_is_clipped_and_stripped() {
        let garbage = format!("ctrl+\u{1b}[31m{}\u{e9}", "x".repeat(40));
        let message = refusal(&garbage);
        assert_eq!(message, format!("Unsupported key: [31m{}", "x".repeat(12)));
        assert!(message.len() <= "Unsupported key: ".len() + MAX_ECHOED_TOKEN_CHARS);
    }

    #[test]
    fn a_token_with_nothing_printable_echoes_a_placeholder() {
        assert_eq!(refusal("ctrl+\u{1b}"), "Unsupported key: ?");
    }
}
