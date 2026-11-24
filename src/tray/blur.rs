//! When the Windows popover closes because the user went somewhere else. Pure decisions over
//! facts the host reads from user32, so they are tested on every OS; `host.rs` gathers the facts
//! and acts on the verdict.

use super::placement::Area;

/// Who holds the foreground after the popover lost it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Foreground {
    /// Nothing does (a transient moment during activation changes).
    Nobody,
    /// A window of this process: the WebView2 child, a context menu.
    Ours,
    /// A taskbar or tray-overflow surface of the shell.
    Shell,
    /// Any other window.
    Other,
}

/// A blur as the host saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Blur {
    /// A mouse button is down: the user clicked somewhere.
    pub button_down: bool,
    pub foreground: Foreground,
    /// Inside the moment after our own show/focus, when Windows can bounce a blur through.
    pub guarded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Hide,
    Keep(&'static str),
}

/// Whether a blur closes the popover. It is transient: it closes when the user goes somewhere
/// else. The guard only swallows blurs no click explains (the bounce our own show/focus
/// produces); a click that took the foreground elsewhere closes it even then, or a click right
/// after opening would leave the popover stranded without focus. A shell surface that
/// activated itself with no button down (a badge refresh, an overflow relayout) is not the
/// user going anywhere.
pub(crate) fn verdict(blur: Blur) -> Verdict {
    match blur.foreground {
        Foreground::Ours => Verdict::Keep("focus stayed in this process"),
        Foreground::Shell if !blur.button_down => Verdict::Keep("taskbar activated itself"),
        _ if blur.guarded && !blur.button_down => Verdict::Keep("guarded: our own show/focus"),
        _ => Verdict::Hide,
    }
}

/// Whether a focus loss closes the macOS popover. AppKit reports the loss but not its cause, so
/// the host only tells whether a mouse button is down. A click elsewhere closes it at once, even
/// right after opening: the show guard only exists for focus bounces no click explains. An app
/// that brought itself forward with no click (a Teams call, a system alert) or a keyboard switch
/// leaves it open, and so does our own show/focus bounce: the host's global mouse-down monitor
/// closes it on the next click outside, which macOS sends no second focus loss for.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn macos_verdict(button_down: bool) -> Verdict {
    if button_down {
        Verdict::Hide
    } else {
        Verdict::Keep("no click: another app activated itself, or our own show/focus")
    }
}

/// How long after a click-outside hide the tray icon's mouse-up still belongs to it.
pub(crate) const TOGGLE_WINDOW_MS: u128 = 600;

/// A hide caused by pressing the mouse outside the popover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PressHide {
    pub x: i32,
    pub y: i32,
}

/// Whether a tray-icon click is the second half of the press that just closed the popover.
/// Pressing the icon while the popover is open hides it (on Windows the foreground moves to the
/// shell; on macOS the outside-click monitor sees the press); the mouse-up then arrives as a
/// click on the icon.
/// Without this the click reopened what the press had just closed, so the icon could never
/// toggle the popover off.
pub(crate) fn is_toggle_close(hide: Option<PressHide>, elapsed_ms: u128, icon: Area) -> bool {
    hide.is_some_and(|press| {
        elapsed_ms <= TOGGLE_WINDOW_MS
            && press.x >= icon.x
            && press.x < icon.x + icon.width
            && press.y >= icon.y
            && press.y < icon.y + icon.height
    })
}

/// A press location in Cocoa points (`NSEvent::mouseLocation`) on the integer grid
/// [`is_toggle_close`] compares. It floors, like [`cocoa_area`]'s origin, so a press on the
/// icon's first point lands inside it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn cocoa_press(x: f64, y: f64) -> PressHide {
    PressHide {
        x: x.floor() as i32,
        y: y.floor() as i32,
    }
}

/// A status item's window frame in Cocoa points, as the [`Area`] [`is_toggle_close`] takes.
/// The origin floors and the far edge ceils, so the fractional frame a scaled display reports
/// is covered whole. Cocoa's y axis points up; `is_toggle_close` only asks whether the press
/// falls inside the box, so that does not matter.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn cocoa_area(x: f64, y: f64, width: f64, height: f64) -> Area {
    let left = x.floor();
    let bottom = y.floor();
    Area {
        x: left as i32,
        y: bottom as i32,
        width: ((x + width).ceil() - left) as i32,
        height: ((y + height).ceil() - bottom) as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ICON: Area = Area {
        x: 3154,
        y: 1392,
        width: 32,
        height: 48,
    };

    fn blur(foreground: Foreground, button_down: bool, guarded: bool) -> Blur {
        Blur {
            button_down,
            foreground,
            guarded,
        }
    }

    #[test]
    fn macos_closes_on_a_click_but_not_when_an_app_activates_itself() {
        // A click outside closes it however soon after opening; no guard delays it.
        assert_eq!(macos_verdict(true), Verdict::Hide);
        assert!(matches!(macos_verdict(false), Verdict::Keep(_)));
    }

    #[test]
    fn a_click_on_another_window_closes_the_popover() {
        assert_eq!(verdict(blur(Foreground::Other, true, false)), Verdict::Hide);
        assert_eq!(
            verdict(blur(Foreground::Other, false, false)),
            Verdict::Hide
        );
    }

    #[test]
    fn a_click_right_after_opening_closes_it_despite_the_guard() {
        // The reported bug: clicking outside within the guard left the popover open and unfocused.
        assert_eq!(verdict(blur(Foreground::Other, true, true)), Verdict::Hide);
        assert_eq!(verdict(blur(Foreground::Shell, true, true)), Verdict::Hide);
        assert_eq!(verdict(blur(Foreground::Nobody, true, true)), Verdict::Hide);
    }

    #[test]
    fn the_guard_still_swallows_the_bounce_of_our_own_show() {
        assert!(matches!(
            verdict(blur(Foreground::Other, false, true)),
            Verdict::Keep(_)
        ));
        assert!(matches!(
            verdict(blur(Foreground::Nobody, false, true)),
            Verdict::Keep(_)
        ));
    }

    #[test]
    fn focus_moving_inside_this_process_never_closes() {
        assert!(matches!(
            verdict(blur(Foreground::Ours, true, false)),
            Verdict::Keep(_)
        ));
    }

    #[test]
    fn the_taskbar_activating_itself_keeps_it_but_a_click_on_it_does_not() {
        assert!(matches!(
            verdict(blur(Foreground::Shell, false, false)),
            Verdict::Keep(_)
        ));
        assert_eq!(verdict(blur(Foreground::Shell, true, false)), Verdict::Hide);
    }

    #[test]
    fn the_icon_mouse_up_after_a_press_on_it_is_a_toggle_close() {
        let press = Some(PressHide { x: 3170, y: 1416 });
        assert!(is_toggle_close(press, 120, ICON));
    }

    #[test]
    fn a_late_or_elsewhere_press_does_not_swallow_the_next_click() {
        let press = Some(PressHide { x: 3170, y: 1416 });
        assert!(!is_toggle_close(press, TOGGLE_WINDOW_MS + 1, ICON));
        let elsewhere = Some(PressHide { x: 1200, y: 700 });
        assert!(!is_toggle_close(elsewhere, 120, ICON));
        assert!(!is_toggle_close(None, 0, ICON));
    }

    #[test]
    fn a_macos_press_on_the_status_item_is_a_toggle_close() {
        // A status item at the top of a 1117-pt-high screen, with the fractional origin and
        // size a scaled display reports. The far edges (1240.7, 1116.6) are fractional so the
        // ceil in `cocoa_area` is what puts the last point inside: x [1203, 1241), y [1093, 1117).
        let icon = cocoa_area(1203.5, 1093.0, 37.2, 23.6);
        assert!(is_toggle_close(
            Some(cocoa_press(1220.4, 1104.7)),
            120,
            icon
        ));
        assert!(is_toggle_close(
            Some(cocoa_press(1203.6, 1093.2)),
            120,
            icon
        ));
        assert!(is_toggle_close(
            Some(cocoa_press(1240.4, 1116.3)),
            120,
            icon
        ));
    }

    #[test]
    fn a_macos_press_below_or_beside_the_status_item_does_not_swallow_the_click() {
        let icon = cocoa_area(1203.5, 1093.0, 37.2, 23.6);
        // Just below the menu bar, inside the popover.
        assert!(!is_toggle_close(
            Some(cocoa_press(1220.0, 1092.5)),
            120,
            icon
        ));
        // Beside it, on another status item.
        assert!(!is_toggle_close(
            Some(cocoa_press(1250.0, 1104.0)),
            120,
            icon
        ));
        assert!(!is_toggle_close(
            Some(cocoa_press(1220.0, 1104.0)),
            TOGGLE_WINDOW_MS + 1,
            icon
        ));
    }
}
