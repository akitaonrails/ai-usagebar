//! The tray icon's right-click menu as a pure model: the popover's Options
//! entries, order and labels. Both hosts build the same menu from it and route
//! the shared item ids back to the same actions; the popover's `menu-labels`
//! IPC swaps the English defaults for its current language.

use serde_json::Value;

/// One selectable entry of the right-click menu, in the popover's Options order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OptionsAction {
    Customize,
    Settings,
    Refresh,
    Detect,
    OpenTui,
    ToggleStartup,
    CheckUpdates,
    About,
    Quit,
}

impl OptionsAction {
    /// Every action, in the menu order (Customize first, Quit last). The hosts
    /// route through `id`/`from_id`; only the tests iterate this list.
    #[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
    pub(crate) const ALL: [OptionsAction; 9] = [
        OptionsAction::Customize,
        OptionsAction::Settings,
        OptionsAction::Refresh,
        OptionsAction::Detect,
        OptionsAction::OpenTui,
        OptionsAction::ToggleStartup,
        OptionsAction::CheckUpdates,
        OptionsAction::About,
        OptionsAction::Quit,
    ];

    /// The muda menu id; the `options-` prefix keeps it clear of the
    /// `fallback-*` ids the emergency menu (#249) matches.
    pub(crate) fn id(self) -> &'static str {
        match self {
            OptionsAction::Customize => "options-customize",
            OptionsAction::Settings => "options-settings",
            OptionsAction::Refresh => "options-refresh",
            OptionsAction::Detect => "options-detect",
            OptionsAction::OpenTui => "options-open-tui",
            OptionsAction::ToggleStartup => "options-startup",
            OptionsAction::CheckUpdates => "options-check-updates",
            OptionsAction::About => "options-about",
            OptionsAction::Quit => "options-quit",
        }
    }

    /// The exact inverse of [`Self::id`]; anything else (including the
    /// fallback menu's ids and a bare entry name) is not this menu.
    pub(crate) fn from_id(id: &str) -> Option<Self> {
        match id {
            "options-customize" => Some(OptionsAction::Customize),
            "options-settings" => Some(OptionsAction::Settings),
            "options-refresh" => Some(OptionsAction::Refresh),
            "options-detect" => Some(OptionsAction::Detect),
            "options-open-tui" => Some(OptionsAction::OpenTui),
            "options-startup" => Some(OptionsAction::ToggleStartup),
            "options-check-updates" => Some(OptionsAction::CheckUpdates),
            "options-about" => Some(OptionsAction::About),
            "options-quit" => Some(OptionsAction::Quit),
            _ => None,
        }
    }

    /// The popover screen this entry navigates to, sent to the page as
    /// `__AIUB_MENU_ACTION__(…)`, or `None` for the entries the host serves
    /// itself (Refresh, Detect, Open TUI, Start at Login, Quit).
    pub(crate) fn popover_action(self) -> Option<&'static str> {
        match self {
            OptionsAction::Customize => Some("customize"),
            OptionsAction::Settings => Some("settings"),
            OptionsAction::CheckUpdates => Some("check-updates"),
            OptionsAction::About => Some("about"),
            _ => None,
        }
    }
}

/// The menu's labels. The English text below is the popover's own; the
/// `menu-labels` IPC replaces them by language, and a refused value keeps
/// whatever the field holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OptionsLabels {
    pub(crate) customize: String,
    pub(crate) settings: String,
    pub(crate) refresh: String,
    pub(crate) detect: String,
    pub(crate) open_tui: String,
    pub(crate) start_at_login: String,
    pub(crate) check_for_updates: String,
    pub(crate) about: String,
    pub(crate) quit: String,
}

impl Default for OptionsLabels {
    fn default() -> Self {
        Self {
            customize: "Customize".into(),
            settings: "Settings".into(),
            refresh: "Refresh".into(),
            detect: "Detect Providers".into(),
            open_tui: "Open TUI".into(),
            start_at_login: "Start at Login".into(),
            // "Check for Updates…" with the U+2026 ellipsis, as the popover shows it.
            check_for_updates: "Check for Updates…".into(),
            about: "About".into(),
            quit: "Quit".into(),
        }
    }
}

impl OptionsLabels {
    /// Fold in the popover's `menu-labels` payload: each field is replaced only
    /// by a usable string (see [`usable`]), so a truncated or hostile send
    /// leaves the current label in place.
    pub(crate) fn merged(&self, value: &Value) -> Self {
        Self {
            customize: usable(value, "customize").unwrap_or_else(|| self.customize.clone()),
            settings: usable(value, "settings").unwrap_or_else(|| self.settings.clone()),
            refresh: usable(value, "refresh").unwrap_or_else(|| self.refresh.clone()),
            detect: usable(value, "detect").unwrap_or_else(|| self.detect.clone()),
            open_tui: usable(value, "openTui").unwrap_or_else(|| self.open_tui.clone()),
            start_at_login: usable(value, "startAtLogin")
                .unwrap_or_else(|| self.start_at_login.clone()),
            check_for_updates: usable(value, "checkForUpdates")
                .unwrap_or_else(|| self.check_for_updates.clone()),
            about: usable(value, "about").unwrap_or_else(|| self.about.clone()),
            quit: usable(value, "quit").unwrap_or_else(|| self.quit.clone()),
        }
    }
}

/// One `menu-labels` field, taken only as a string that trims to at least one
/// of at most 64 chars, none of them a control character.
fn usable(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .filter(|text| text.chars().count() <= 64)
        .filter(|text| !text.chars().any(char::is_control))
        .map(str::to_owned)
}

/// One row of the right-click menu, in display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OptionsEntry {
    Item {
        action: OptionsAction,
        label: String,
    },
    Check {
        action: OptionsAction,
        label: String,
        checked: bool,
    },
    Separator,
}

/// The menu rows: the popover's Options order, with Customize only in the
/// Classic style (Native has no Customize screen) and Start at Login as a
/// check marked by the real login-item state.
pub(crate) fn options_entries(
    labels: &OptionsLabels,
    native: bool,
    startup_enabled: bool,
) -> Vec<OptionsEntry> {
    let mut entries = Vec::new();
    if !native {
        entries.push(OptionsEntry::Item {
            action: OptionsAction::Customize,
            label: labels.customize.clone(),
        });
    }
    entries.push(OptionsEntry::Item {
        action: OptionsAction::Settings,
        label: labels.settings.clone(),
    });
    entries.push(OptionsEntry::Separator);
    entries.push(OptionsEntry::Item {
        action: OptionsAction::Refresh,
        label: labels.refresh.clone(),
    });
    entries.push(OptionsEntry::Item {
        action: OptionsAction::Detect,
        label: labels.detect.clone(),
    });
    entries.push(OptionsEntry::Item {
        action: OptionsAction::OpenTui,
        label: labels.open_tui.clone(),
    });
    entries.push(OptionsEntry::Separator);
    entries.push(OptionsEntry::Check {
        action: OptionsAction::ToggleStartup,
        label: labels.start_at_login.clone(),
        checked: startup_enabled,
    });
    entries.push(OptionsEntry::Separator);
    entries.push(OptionsEntry::Item {
        action: OptionsAction::CheckUpdates,
        label: labels.check_for_updates.clone(),
    });
    entries.push(OptionsEntry::Item {
        action: OptionsAction::About,
        label: labels.about.clone(),
    });
    entries.push(OptionsEntry::Item {
        action: OptionsAction::Quit,
        label: labels.quit.clone(),
    });
    entries
}

/// Materialize the rows into a live muda `Menu`: everything it holds is
/// removed first, so one object is refilled in place instead of replaced —
/// the Windows host's subclass is attached to it and cannot be replaced.
#[cfg(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", feature = "linux-tray")
))]
pub(crate) fn fill_menu(menu: &tray_icon::menu::Menu, entries: &[OptionsEntry]) {
    use tray_icon::menu::{CheckMenuItem, MenuItem, PredefinedMenuItem};

    while menu.remove_at(0).is_some() {}
    for entry in entries {
        let _ = match entry {
            OptionsEntry::Item { action, label } => {
                menu.append(&MenuItem::with_id(action.id(), label, true, None))
            }
            OptionsEntry::Check {
                action,
                label,
                checked,
            } => menu.append(&CheckMenuItem::with_id(
                action.id(),
                label,
                true,
                *checked,
                None,
            )),
            OptionsEntry::Separator => menu.append(&PredefinedMenuItem::separator()),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(action: OptionsAction, label: &str) -> OptionsEntry {
        OptionsEntry::Item {
            action,
            label: label.into(),
        }
    }

    #[test]
    fn classic_menu_matches_the_popover_options_order() {
        assert_eq!(
            options_entries(&OptionsLabels::default(), false, false),
            vec![
                item(OptionsAction::Customize, "Customize"),
                item(OptionsAction::Settings, "Settings"),
                OptionsEntry::Separator,
                item(OptionsAction::Refresh, "Refresh"),
                item(OptionsAction::Detect, "Detect Providers"),
                item(OptionsAction::OpenTui, "Open TUI"),
                OptionsEntry::Separator,
                OptionsEntry::Check {
                    action: OptionsAction::ToggleStartup,
                    label: "Start at Login".into(),
                    checked: false,
                },
                OptionsEntry::Separator,
                item(OptionsAction::CheckUpdates, "Check for Updates…"),
                item(OptionsAction::About, "About"),
                item(OptionsAction::Quit, "Quit"),
            ]
        );
    }

    #[test]
    fn native_menu_omits_customize() {
        let classic = options_entries(&OptionsLabels::default(), false, false);
        let native = options_entries(&OptionsLabels::default(), true, false);
        assert_eq!(native, classic[1..].to_vec());
    }

    #[test]
    fn startup_check_follows_the_login_item() {
        let checked = |entries: &[OptionsEntry]| {
            entries.iter().find_map(|entry| match entry {
                OptionsEntry::Check {
                    action: OptionsAction::ToggleStartup,
                    checked,
                    ..
                } => Some(*checked),
                _ => None,
            })
        };
        assert_eq!(
            checked(&options_entries(&OptionsLabels::default(), false, false)),
            Some(false)
        );
        assert_eq!(
            checked(&options_entries(&OptionsLabels::default(), false, true)),
            Some(true)
        );
    }

    #[test]
    fn ids_round_trip_and_do_not_collide_with_the_fallback_menu() {
        for action in OptionsAction::ALL {
            assert_eq!(OptionsAction::from_id(action.id()), Some(action));
        }
        let ids: Vec<&str> = OptionsAction::ALL
            .iter()
            .map(|action| action.id())
            .collect();
        assert_eq!(
            ids.iter().collect::<std::collections::HashSet<_>>().len(),
            ids.len()
        );
        assert_eq!(OptionsAction::from_id("fallback-refresh"), None);
        assert_eq!(OptionsAction::from_id("refresh"), None);
        assert_eq!(OptionsAction::from_id(""), None);
    }

    #[test]
    fn popover_actions_cover_only_screen_entries() {
        assert_eq!(OptionsAction::Customize.popover_action(), Some("customize"));
        assert_eq!(OptionsAction::Settings.popover_action(), Some("settings"));
        assert_eq!(
            OptionsAction::CheckUpdates.popover_action(),
            Some("check-updates")
        );
        assert_eq!(OptionsAction::About.popover_action(), Some("about"));
        for action in [
            OptionsAction::Refresh,
            OptionsAction::Detect,
            OptionsAction::OpenTui,
            OptionsAction::ToggleStartup,
            OptionsAction::Quit,
        ] {
            assert_eq!(action.popover_action(), None);
        }
    }

    #[test]
    fn merged_takes_usable_labels_and_keeps_the_rest() {
        let merged = OptionsLabels::default().merged(&serde_json::json!({
            "cmd": "menu-labels",
            "settings": "Configurações",
            "quit": "  Sair  ",
            "refresh": "",
            "detect": "a\nb",
            "about": 42,
            "openTui": "a".repeat(65),
        }));
        assert_eq!(merged.settings, "Configurações");
        assert_eq!(merged.quit, "Sair");
        // Refused values (empty, control char, not a string, too long) keep
        // the current, default label.
        assert_eq!(merged.refresh, "Refresh");
        assert_eq!(merged.detect, "Detect Providers");
        assert_eq!(merged.about, "About");
        assert_eq!(merged.open_tui, "Open TUI");
        // Absent fields keep the default.
        assert_eq!(merged.customize, "Customize");
        assert_eq!(merged.start_at_login, "Start at Login");
        assert_eq!(merged.check_for_updates, "Check for Updates…");
    }
}
