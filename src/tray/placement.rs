//! Where the Windows popover goes on screen. Pure geometry in physical pixels, so it is tested
//! on every OS; `host.rs` only feeds it the monitor's work area (`rcWork`: the monitor minus the
//! taskbar and docked app bars).

/// A screen rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Area {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Area {
    pub(crate) fn right(self) -> i32 {
        self.x + self.width
    }

    pub(crate) fn bottom(self) -> i32 {
        self.y + self.height
    }
}

/// Space kept between the popover and each work-area edge, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Insets {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Insets {
    /// `margin` on every edge except the ones the taskbar was docked against, which get none:
    /// the popover sits right by the taskbar, and the window's invisible frame border is the
    /// room its shadow draws in. Which edges those are shows in how the work area was pulled in
    /// from the monitor.
    pub(crate) fn by_taskbar(monitor: Area, work: Area, margin: i32) -> Self {
        let edge = |pulled_in: bool| if pulled_in { 0 } else { margin };
        Self {
            left: edge(work.x > monitor.x),
            top: edge(work.y > monitor.y),
            right: edge(work.right() < monitor.right()),
            bottom: edge(work.bottom() < monitor.bottom()),
        }
    }
}

/// Moves a `width`×`height` window at (`x`, `y`) inside `work`, `insets` clear of each edge.
/// A window larger than the area keeps its top-left corner on the area's inset corner.
pub(crate) fn clamp_into(
    work: Area,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    insets: Insets,
) -> (i32, i32) {
    let min_x = work.x + insets.left;
    let min_y = work.y + insets.top;
    let max_x = work.right() - width - insets.right;
    let max_y = work.bottom() - height - insets.bottom;
    (
        x.clamp(min_x, max_x.max(min_x)),
        y.clamp(min_y, max_y.max(min_y)),
    )
}

/// The monitor edge the taskbar is docked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskbarEdge {
    Bottom,
    Top,
    Left,
    Right,
}

impl TaskbarEdge {
    /// The edge `icon` hangs from. An icon on the taskbar itself lies outside the work area,
    /// on the taskbar's side. One inside it (the overflow flyout, an auto-hidden taskbar) falls
    /// back to the edge the work area was pulled in from, then to the monitor edge nearest it.
    pub(crate) fn of(monitor: Area, work: Area, icon: Area) -> Self {
        let cx = icon.x + icon.width / 2;
        let cy = icon.y + icon.height / 2;
        if cy >= work.bottom() {
            return Self::Bottom;
        }
        if cy < work.y {
            return Self::Top;
        }
        if cx < work.x {
            return Self::Left;
        }
        if cx >= work.right() {
            return Self::Right;
        }
        if work.bottom() < monitor.bottom() {
            return Self::Bottom;
        }
        if work.y > monitor.y {
            return Self::Top;
        }
        if work.x > monitor.x {
            return Self::Left;
        }
        if work.right() < monitor.right() {
            return Self::Right;
        }
        [
            (monitor.bottom() - cy, Self::Bottom),
            (cy - monitor.y, Self::Top),
            (cx - monitor.x, Self::Left),
            (monitor.right() - cx, Self::Right),
        ]
        .into_iter()
        .min_by_key(|(distance, _)| *distance)
        .map_or(Self::Bottom, |(_, edge)| edge)
    }
}

/// Where the popover opens for a click on the tray `icon`: centered on it, on the side away from
/// the taskbar. An icon on the taskbar puts the popover right on the work-area edge, exactly
/// where the global shortcut puts it; an icon inside the work area (the overflow flyout) keeps
/// `margin` from it. Either way the result stays on the work area.
pub(crate) fn beside_icon(
    monitor: Area,
    work: Area,
    icon: Area,
    width: i32,
    height: i32,
    margin: i32,
) -> (i32, i32) {
    let centered_x = icon.x + icon.width / 2 - width / 2;
    let centered_y = icon.y + icon.height / 2 - height / 2;
    let (x, y) = match TaskbarEdge::of(monitor, work, icon) {
        TaskbarEdge::Bottom if icon.y >= work.bottom() => (centered_x, work.bottom() - height),
        TaskbarEdge::Bottom => (centered_x, icon.y - height - margin),
        TaskbarEdge::Top if icon.bottom() <= work.y => (centered_x, work.y),
        TaskbarEdge::Top => (centered_x, icon.bottom() + margin),
        TaskbarEdge::Left if icon.right() <= work.x => (work.x, centered_y),
        TaskbarEdge::Left => (icon.right() + margin, centered_y),
        TaskbarEdge::Right if icon.x >= work.right() => (work.right() - width, centered_y),
        TaskbarEdge::Right => (icon.x - width - margin, centered_y),
    };
    clamp_into(
        work,
        x,
        y,
        width,
        height,
        Insets::by_taskbar(monitor, work, margin),
    )
}

/// Where the popover opens with no tray icon to hang from (the global shortcut): the work area's
/// corner nearest the taskbar. A taskbar at the bottom or right (the default) gives the
/// bottom-right corner, where the tray sits.
pub(crate) fn corner_near_taskbar(
    monitor: Area,
    work: Area,
    width: i32,
    height: i32,
    insets: Insets,
) -> (i32, i32) {
    let x = if work.x > monitor.x {
        work.x + insets.left
    } else {
        work.right() - width - insets.right
    };
    let y = if work.y > monitor.y {
        work.y + insets.top
    } else {
        work.bottom() - height - insets.bottom
    };
    clamp_into(work, x, y, width, height, insets)
}

/// Tallest logical inner height that keeps the whole window inside `work`: the window frame
/// (`outer_height - inner_height`, physical) is taken off first, so the frame cannot push the
/// window's bottom edge past the work area.
pub(crate) fn max_inner_height(work: Area, frame_height: i32, scale: f64) -> f64 {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    f64::from((work.height - frame_height.max(0)).max(0)) / scale
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reported case: a 3440×1440 monitor with a 48 px taskbar along the bottom.
    const MONITOR: Area = Area {
        x: 0,
        y: 0,
        width: 3440,
        height: 1440,
    };
    const WORK: Area = Area {
        x: 0,
        y: 0,
        width: 3440,
        height: 1392,
    };

    const EVEN: Insets = Insets {
        left: 8,
        top: 8,
        right: 8,
        bottom: 8,
    };

    #[test]
    fn only_the_taskbar_edge_loses_its_margin() {
        assert_eq!(
            Insets::by_taskbar(MONITOR, WORK, 8),
            Insets {
                left: 8,
                top: 8,
                right: 8,
                bottom: 0
            }
        );
        let top = Area {
            x: 0,
            y: 48,
            width: 3440,
            height: 1392,
        };
        assert_eq!(Insets::by_taskbar(MONITOR, top, 8).top, 0);
        assert_eq!(Insets::by_taskbar(MONITOR, top, 8).bottom, 8);
        // Auto-hidden taskbar: nothing pulled in, a margin all round.
        assert_eq!(Insets::by_taskbar(MONITOR, MONITOR, 8), EVEN);
    }

    #[test]
    fn a_tall_popover_stays_above_the_taskbar() {
        // Hanging above the tray icon, a too-tall window would start off-screen; it is pulled
        // down to the top margin and its bottom may reach the taskbar but not cross it.
        let insets = Insets::by_taskbar(MONITOR, WORK, 8);
        let (x, y) = clamp_into(WORK, 3108, -500, 316, 1384, insets);
        assert_eq!((x, y), (3108, 8));
        assert!(y + 1384 <= WORK.bottom());
    }

    #[test]
    fn a_window_below_the_work_area_sits_right_on_the_taskbar() {
        let insets = Insets::by_taskbar(MONITOR, WORK, 8);
        let (_, y) = clamp_into(WORK, 3108, 1200, 316, 400, insets);
        assert_eq!(y, 1392 - 400);
    }

    #[test]
    fn a_window_larger_than_the_work_area_pins_to_its_top_left_inset() {
        assert_eq!(clamp_into(WORK, 50, 50, 4000, 2000, EVEN), (8, 8));
    }

    #[test]
    fn the_shortcut_opens_by_the_tray_for_a_bottom_taskbar() {
        let insets = Insets::by_taskbar(MONITOR, WORK, 8);
        assert_eq!(
            corner_near_taskbar(MONITOR, WORK, 316, 600, insets),
            (3440 - 316 - 8, 1392 - 600)
        );
    }

    #[test]
    fn the_shortcut_follows_a_taskbar_moved_to_the_top_or_left() {
        let top = Area {
            x: 0,
            y: 48,
            width: 3440,
            height: 1392,
        };
        let insets = Insets::by_taskbar(MONITOR, top, 8);
        assert_eq!(
            corner_near_taskbar(MONITOR, top, 316, 600, insets),
            (3440 - 316 - 8, 48)
        );
        let left = Area {
            x: 60,
            y: 0,
            width: 3380,
            height: 1440,
        };
        let insets = Insets::by_taskbar(MONITOR, left, 8);
        assert_eq!(
            corner_near_taskbar(MONITOR, left, 316, 600, insets),
            (60, 1440 - 600 - 8)
        );
    }

    #[test]
    fn a_secondary_monitor_keeps_its_own_origin() {
        let monitor = Area {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let work = Area {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1040,
        };
        let insets = Insets::by_taskbar(monitor, work, 8);
        assert_eq!(
            corner_near_taskbar(monitor, work, 316, 600, insets),
            (-316 - 8, 1040 - 600)
        );
    }

    /// The popover's icon on a bottom taskbar, as UI Automation reported it.
    const TASKBAR_ICON: Area = Area {
        x: 3154,
        y: 1392,
        width: 32,
        height: 48,
    };

    #[test]
    fn a_taskbar_icon_click_opens_right_on_the_taskbar_like_the_shortcut() {
        // Centered on the icon, bottom edge on the work area: the same spot the shortcut uses,
        // not the icon's top minus a margin, which left twice the gap.
        assert_eq!(
            beside_icon(MONITOR, WORK, TASKBAR_ICON, 316, 600, 8),
            (3170 - 158, 1392 - 600)
        );
    }

    #[test]
    fn an_overflow_icon_keeps_the_margin_above_it() {
        // The overflow flyout sits inside the work area, so the popover hangs above the icon.
        let icon = Area {
            x: 3038,
            y: 1255,
            width: 40,
            height: 40,
        };
        assert_eq!(TaskbarEdge::of(MONITOR, WORK, icon), TaskbarEdge::Bottom);
        assert_eq!(
            beside_icon(MONITOR, WORK, icon, 316, 600, 8),
            (3058 - 158, 1255 - 600 - 8)
        );
    }

    #[test]
    fn a_top_taskbar_icon_opens_below_the_taskbar() {
        let work = Area {
            x: 0,
            y: 48,
            width: 3440,
            height: 1392,
        };
        let icon = Area {
            y: 0,
            ..TASKBAR_ICON
        };
        assert_eq!(TaskbarEdge::of(MONITOR, work, icon), TaskbarEdge::Top);
        assert_eq!(
            beside_icon(MONITOR, work, icon, 316, 600, 8),
            (3170 - 158, 48)
        );
    }

    #[test]
    fn a_side_taskbar_icon_opens_beside_the_taskbar() {
        let left = Area {
            x: 60,
            y: 0,
            width: 3380,
            height: 1440,
        };
        let icon = Area {
            x: 0,
            y: 1300,
            width: 60,
            height: 40,
        };
        assert_eq!(TaskbarEdge::of(MONITOR, left, icon), TaskbarEdge::Left);
        // Centered on the icon vertically, then held 8 px off the bottom edge.
        assert_eq!(
            beside_icon(MONITOR, left, icon, 316, 600, 8),
            (60, 1440 - 600 - 8)
        );

        let right = Area {
            x: 0,
            y: 0,
            width: 3380,
            height: 1440,
        };
        let icon = Area { x: 3380, ..icon };
        assert_eq!(TaskbarEdge::of(MONITOR, right, icon), TaskbarEdge::Right);
        assert_eq!(
            beside_icon(MONITOR, right, icon, 316, 600, 8),
            (3380 - 316, 1440 - 600 - 8)
        );
    }

    #[test]
    fn an_auto_hidden_taskbar_uses_the_monitor_edge_nearest_the_icon() {
        let icon = Area {
            x: 3154,
            y: 1400,
            width: 32,
            height: 40,
        };
        assert_eq!(TaskbarEdge::of(MONITOR, MONITOR, icon), TaskbarEdge::Bottom);
        assert_eq!(
            beside_icon(MONITOR, MONITOR, icon, 316, 600, 8),
            (3170 - 158, 1400 - 600 - 8)
        );
    }

    #[test]
    fn a_tall_popover_from_the_icon_still_fits_the_work_area() {
        let (x, y) = beside_icon(MONITOR, WORK, TASKBAR_ICON, 316, 1384, 8);
        assert_eq!((x, y), (3012, 8));
        assert!(y + 1384 <= WORK.bottom());
    }

    #[test]
    fn the_height_budget_leaves_room_for_the_frame() {
        // 1392 px of work area, a 9 px frame, 100% scale: the inner height may use 1383.
        assert_eq!(max_inner_height(WORK, 9, 1.0), 1383.0);
        // At 150% the same budget is 922 logical px.
        assert_eq!(max_inner_height(WORK, 9, 1.5), 922.0);
    }

    #[test]
    fn a_nonsense_scale_or_frame_is_read_as_none() {
        assert_eq!(max_inner_height(WORK, -5, 0.0), 1392.0);
        assert_eq!(max_inner_height(WORK, 0, f64::NAN), 1392.0);
    }
}
