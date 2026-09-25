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

/// How long after a click-outside hide the tray icon's mouse-up still belongs to it.
pub(crate) const TOGGLE_WINDOW_MS: u128 = 600;

/// A hide caused by pressing the mouse outside the popover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PressHide {
    pub x: i32,
    pub y: i32,
}

/// Whether a tray-icon click is the second half of the press that just closed the popover.
/// Pressing the icon while the popover is open takes the foreground to the shell and hides it;
/// the mouse-up then arrives as a click on the icon. Without this the click reopened what the
/// press had just closed, so the icon could never toggle the popover off.
pub(crate) fn is_toggle_close(hide: Option<PressHide>, elapsed_ms: u128, icon: Area) -> bool {
    hide.is_some_and(|press| {
        elapsed_ms <= TOGGLE_WINDOW_MS
            && press.x >= icon.x
            && press.x < icon.x + icon.width
            && press.y >= icon.y
            && press.y < icon.y + icon.height
    })
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
}
