//! Shared popover geometry. Pure numbers so Linux CI can test the clamp
//! without compiling a WebView host.
#![allow(dead_code)]

/// Width of the macOS provider switcher in logical points.
pub const WINDOW_WIDTH: f64 = 390.0;
/// Initial height only: the web content drives it afterwards via `resize`.
pub const WINDOW_HEIGHT: f64 = 420.0;
/// Smallest height a `resize` request can shrink the popover to.
/// Sized so the footer Options menu (nine rows, opens upward) fits without
/// Radix scrolling the list on short screens like Customize / provider detail.
pub const MIN_POPOVER_HEIGHT: f64 = 360.0;
/// Breathing room kept between the popover and the monitor's edges.
pub const WORK_AREA_MARGIN: f64 = 16.0;
/// Gap between the bottom of the menu bar and the top of the popover.
/// Zero so the panel hangs flush under the status item, like a menu.
pub const POPOVER_TOP_GAP: f64 = 0.0;
/// Gap between the popover and the bottom of the visible screen (dock / desktop).
pub const POPOVER_BOTTOM_GAP: f64 = 24.0;
/// Horizontal inset from the visible screen edges.
pub const POPOVER_SIDE_MARGIN: f64 = 8.0;
/// Tall dashboards stop short of filling the column, matching OpenUsage.
pub const POPOVER_MAX_HEIGHT_FRACTION: f64 = 0.85;
/// Used when `visibleFrame` does not subtract the menu bar (accessory apps).
pub const MENU_BAR_MIN_HEIGHT: f64 = 24.0;
/// Used when no monitor can be resolved at all.
pub const FALLBACK_WORK_AREA_HEIGHT: f64 = 800.0;
/// Absorb the mouse-up that opened the popover so it cannot hit the footer.
pub const CLICK_LOCK_MS: u64 = 400;
/// Opaque WebView background (WebView2 ignores translucency; WKWebView matches).
pub const LIGHT_BACKGROUND: (u8, u8, u8, u8) = (255, 255, 255, 255);
pub const DARK_BACKGROUND: (u8, u8, u8, u8) = (30, 30, 30, 255);
/// Corner radius of the panel surface; tuned to read like a system popover.
pub const CORNER_RADIUS: f64 = 13.0;

/// Height the popover may grow to for `requested` logical px: rounded, never
/// below [`MIN_POPOVER_HEIGHT`], never past the work area minus its margin.
pub fn clamp_popover_height(requested: f64, work_area_height: f64) -> f64 {
    let max = (work_area_height - WORK_AREA_MARGIN).max(MIN_POPOVER_HEIGHT);
    requested.round().clamp(MIN_POPOVER_HEIGHT, max)
}

/// Axis-aligned rectangle in Cocoa screen space: origin at the bottom-left of
/// the primary display, y increasing up, units in points (not pixels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CocoaRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl CocoaRect {
    pub fn contains(self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    pub fn max_x(self) -> f64 {
        self.x + self.w
    }

    pub fn max_y(self) -> f64 {
        self.y + self.h
    }
}

/// Inputs for [`cocoa_popover_frame`]. `below_y` is the Cocoa min-Y of the
/// menu bar (bottom edge of `NSStatusBarWindow`); the popover hangs
/// [`POPOVER_TOP_GAP`] under that and keeps [`POPOVER_BOTTOM_GAP`] above the
/// visible bottom (dock / desktop).
#[derive(Debug, Clone, Copy)]
pub struct PopoverPlacement {
    pub visible: CocoaRect,
    pub below_y: f64,
    pub icon_x: f64,
    pub popover_w: f64,
    pub popover_h: f64,
}

/// Infer the Cocoa Y of the menu-bar's bottom edge when we cannot see the
/// status-item window. `visibleFrame` *should* already exclude the bar; some
/// accessory apps get `visibleFrame == frame`, so we never trust a zero inset.
pub fn menu_bar_bottom_y(screen: CocoaRect, visible: CocoaRect) -> f64 {
    let inset = (screen.max_y() - visible.max_y()).max(0.0);
    screen.max_y() - inset.max(MENU_BAR_MIN_HEIGHT)
}

/// Popover frame in Cocoa coordinates (origin bottom-left, y up, points).
pub fn cocoa_popover_frame(p: PopoverPlacement) -> CocoaRect {
    let top = p.below_y - POPOVER_TOP_GAP;
    let bottom = p.visible.y + POPOVER_BOTTOM_GAP;
    let available = (top - bottom).max(1.0);
    let max_h = (available * POPOVER_MAX_HEIGHT_FRACTION).min(available);
    let h = p.popover_h.min(max_h).max(1.0);
    let max_w = (p.visible.w - 2.0 * POPOVER_SIDE_MARGIN).max(1.0);
    let w = p.popover_w.min(max_w);
    let min_x = p.visible.x + POPOVER_SIDE_MARGIN;
    let max_x = p.visible.max_x() - POPOVER_SIDE_MARGIN - w;
    let x = (p.icon_x - w / 2.0).clamp(min_x, max_x.max(min_x));
    let y = (top - h).max(bottom);
    CocoaRect { x, y, w, h }
}

#[cfg(test)]
mod tests {
    use super::{
        CocoaRect, MENU_BAR_MIN_HEIGHT, MIN_POPOVER_HEIGHT, POPOVER_BOTTOM_GAP,
        POPOVER_SIDE_MARGIN, POPOVER_TOP_GAP, PopoverPlacement, WORK_AREA_MARGIN,
        clamp_popover_height, cocoa_popover_frame, menu_bar_bottom_y,
    };

    fn place(visible: CocoaRect, icon_x: f64, h: f64) -> CocoaRect {
        cocoa_popover_frame(PopoverPlacement {
            visible,
            below_y: menu_bar_bottom_y(visible, visible),
            icon_x,
            popover_w: 320.0,
            popover_h: h,
        })
    }

    #[test]
    fn clamp_raises_requests_below_the_minimum() {
        assert_eq!(clamp_popover_height(40.0, 1080.0), MIN_POPOVER_HEIGHT);
    }

    #[test]
    fn clamp_keeps_requests_within_range_rounded() {
        assert_eq!(clamp_popover_height(512.4, 1080.0), 512.0);
        assert_eq!(clamp_popover_height(512.6, 1080.0), 513.0);
    }

    #[test]
    fn clamp_caps_requests_at_work_area_minus_margin() {
        assert_eq!(
            clamp_popover_height(5000.0, 1080.0),
            1080.0 - WORK_AREA_MARGIN
        );
    }

    #[test]
    fn clamp_never_shrinks_below_minimum_on_tiny_work_area() {
        assert_eq!(clamp_popover_height(300.0, 100.0), MIN_POPOVER_HEIGHT);
        assert_eq!(clamp_popover_height(50.0, 10.0), MIN_POPOVER_HEIGHT);
    }

    fn primary() -> CocoaRect {
        CocoaRect {
            x: 0.0,
            y: 0.0,
            w: 1440.0,
            h: 900.0,
        }
    }

    fn right_of_primary() -> CocoaRect {
        CocoaRect {
            x: 1440.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        }
    }

    #[test]
    fn popover_on_a_right_hand_screen_does_not_jump_to_the_primary() {
        let visible = right_of_primary();
        let frame = place(visible, 2400.0, 420.0);
        assert!(
            frame.x >= visible.x,
            "left edge {} must stay on the secondary (origin {})",
            frame.x,
            visible.x
        );
        assert!(frame.max_x() <= visible.max_x() + 0.01);
        let expected_top = menu_bar_bottom_y(visible, visible) - POPOVER_TOP_GAP;
        assert!((frame.max_y() - expected_top).abs() < 0.01);
    }

    #[test]
    fn popover_near_the_right_edge_is_clamped_onto_that_screen() {
        let visible = right_of_primary();
        let frame = place(visible, 1440.0 + 1910.0, 420.0);
        assert!((frame.max_x() - (visible.max_x() - POPOVER_SIDE_MARGIN)).abs() < 0.01);
        assert!(frame.x >= visible.x + POPOVER_SIDE_MARGIN);
    }

    #[test]
    fn popover_on_a_left_hand_screen_keeps_a_negative_origin() {
        let left = CocoaRect {
            x: -1920.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        let frame = place(left, -200.0, 420.0);
        assert!(frame.x >= left.x);
        assert!(frame.max_x() <= left.max_x() + 0.01);
        assert!(frame.x < 0.0, "must not be shifted onto the primary");
    }

    #[test]
    fn a_tall_popover_keeps_a_gap_above_the_bottom_of_the_screen() {
        let visible = primary();
        let frame = place(visible, 200.0, 5000.0);
        assert!(frame.y >= visible.y + POPOVER_BOTTOM_GAP - 0.01);
        assert!(frame.max_y() <= visible.max_y() - MENU_BAR_MIN_HEIGHT + 0.01);
        assert!(frame.h < visible.h);
    }

    #[test]
    fn accessory_visible_frame_still_clears_the_menu_bar() {
        let screen = primary();
        // Accessory apps sometimes see visibleFrame == frame (no menu-bar inset).
        let below = menu_bar_bottom_y(screen, screen);
        assert_eq!(below, screen.max_y() - MENU_BAR_MIN_HEIGHT);
        let frame = cocoa_popover_frame(PopoverPlacement {
            visible: screen,
            below_y: below,
            icon_x: 200.0,
            popover_w: 320.0,
            popover_h: 420.0,
        });
        assert!(
            (frame.max_y() - (below - POPOVER_TOP_GAP)).abs() < 0.01,
            "popover top {} must sit flush under the menu bar at {}",
            frame.max_y(),
            below
        );
    }

    #[test]
    fn popover_top_is_flush_with_the_menu_bar() {
        let visible = primary();
        let below = menu_bar_bottom_y(visible, visible);
        let frame = place(visible, 200.0, 420.0);
        assert_eq!(frame.max_y(), below);
        assert!(frame.max_y() < visible.max_y());
    }
}
