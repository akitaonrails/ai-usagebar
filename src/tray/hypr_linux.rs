//! Where the Linux popover opens on Hyprland.
//!
//! Wayland lets a client say how big its window is, never where it goes, so
//! `set_outer_position` is ignored and Hyprland centers a floating window. On
//! Hyprland the host asks the compositor instead, through `hyprctl`:
//!
//! 1. the cursor position at the moment of the click is the anchor (the tray
//!    protocol gives the host no icon rectangle);
//! 2. the monitor under the anchor, its reserved area (the bar) and the user's
//!    gaps give the popover's frame, flush with the bar;
//! 3. a window rule for this popover's class and title carries that frame, so
//!    the compositor maps the window already in place; a window that is already
//!    mapped (a resize while open) is moved and resized through its address.
//!
//! Every `hyprctl` runs on the worker thread started by [`Positioner::start`],
//! by absolute path, without a shell and under a deadline, never on the GTK
//! thread. The cursor is read on the tray thread, where the click arrives. Each
//! request carries a [`Generation`]; a result for an opening that has since
//! been closed or replaced is dropped, and a dispatch is skipped when the
//! generation moved between the check and the effect (the window between those
//! two is as wide as one `hyprctl` call and is not closed: the next
//! `advance` makes the following request win).
//!
//! The frame, the Lua, the parsers and the worker loop are pure, so the tests
//! never run `hyprctl` or read `$HOME`. A failure here is a warning on stderr
//! and the popover opens where the compositor put it. Everything is in
//! Hyprland's logical (layout) pixels, the unit of `hyprctl monitors`.
//!
//! Compositors other than Hyprland (no `HYPRLAND_INSTANCE_SIGNATURE`) get no
//! positioner and keep the previous behavior.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::panel;

/// Window class GTK gives the popover (the binary name).
pub(super) const CLASS: &str = "ai-usagebar-tray";
/// The popover's window title.
pub(super) const TITLE: &str = "AI Usage";
/// Name of the window rule, so a re-registration replaces the previous one.
const RULE_NAME: &str = "ai-usagebar-tray";

const HYPRCTL: &str = "/usr/bin/hyprctl";
const TIMEOUT: Duration = Duration::from_secs(2);
/// Cap on what a `hyprctl` call may print before the call is an error.
const MAX_OUTPUT: u64 = 8 * 1024 * 1024;
/// Space the popover keeps clear of a monitor edge when no gap is known.
const DEFAULT_MARGIN: f64 = 8.0;
/// How many 20 ms polls wait for the mapped window to show up in `clients`.
const WINDOW_POLLS: u32 = 25;
/// How many times a resize that Hyprland did not take is dispatched again, and how
/// long to wait before looking: a bounded wait for GTK to let go of the old fixed
/// size, never a guess that it already has.
const SETTLE_RETRIES: u32 = 8;
const SETTLE_PAUSE: Duration = Duration::from_millis(40);
/// Rounding of fractional scales moves a window edge by a pixel.
const SETTLE_TOLERANCE: f64 = 2.0;

/// Where the cursor was, in logical layout pixels.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub(super) struct Anchor {
    pub(super) x: f64,
    pub(super) y: f64,
}

/// One entry of `hyprctl monitors -j`.
#[derive(Debug, Deserialize)]
pub(super) struct Monitor {
    #[serde(default)]
    name: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale: f64,
    /// Space taken by layer surfaces (the bar): left, top, right, bottom.
    reserved: [f64; 4],
}

/// Where the popover goes, in global layout pixels, and its size.
#[derive(Debug, PartialEq)]
pub(super) struct Frame {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: u32,
    pub(super) height: u32,
}

/// One entry of `hyprctl clients -j`, the fields placement needs.
#[derive(Debug, Deserialize)]
pub(super) struct Client {
    pub(super) address: String,
    pub(super) pid: u32,
    pub(super) title: String,
    pub(super) floating: bool,
    pub(super) pinned: bool,
    /// Top-left corner, in global layout pixels.
    pub(super) at: [f64; 2],
    pub(super) size: [f64; 2],
}

/// What the host asks the worker to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PlacementRequest {
    pub(super) generation: u64,
    /// Generation of the opening this request belongs to: a resize reuses the
    /// anchor its opening captured, a new opening captures again.
    opening: u64,
    pub(super) is_opening: bool,
    pub(super) anchor: Option<Anchor>,
    /// Logical width the popover should have.
    pub(super) width: u32,
    /// Logical height the page asked for.
    pub(super) requested: u32,
}

pub(super) fn click_request(
    generation: u64,
    anchor: Option<Anchor>,
    width: u32,
    requested: u32,
) -> PlacementRequest {
    PlacementRequest {
        generation,
        opening: generation,
        is_opening: true,
        anchor,
        width,
        requested,
    }
}

pub(super) fn resize_request(
    generation: u64,
    opening: u64,
    anchor: Option<Anchor>,
    width: u32,
    requested: u32,
) -> PlacementRequest {
    PlacementRequest {
        generation,
        opening,
        is_opening: false,
        anchor,
        width,
        requested,
    }
}

/// What a reply carries, only when it answers the newest request.
pub(super) fn applied_height<T>(current: u64, received: u64, value: T) -> Option<T> {
    (current == received).then_some(value)
}

/// Whether the session is Hyprland.
pub(super) fn hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

/// Whether Omarchy's shell is installed: through `OMARCHY_PATH` when set, else at
/// the default location. Omarchy's bar floats the popovers a gap away from it.
pub(super) fn omarchy_detected(env_path: Option<&Path>, default: &Path) -> bool {
    env_path
        .filter(|path| !path.as_os_str().is_empty())
        .is_some_and(|path| path.join("shell/shell.qml").is_file())
        || default.join("shell/shell.qml").is_file()
}

/// A warning on stderr. What `hyprctl` printed is data, not a terminal program.
fn warn(context: &str, detail: &str) {
    eprintln!(
        "ai-usagebar-tray: {context}: {}",
        crate::display::sanitize_untrusted_line(detail)
    );
}

/// Reads a pipe to its end, or to `MAX_OUTPUT + 1` bytes: the one byte past the
/// cap is how an output that is too long is told from one that is exactly at it.
/// A reader that stopped at the cap would announce EOF while the child had more
/// to say, and the caller would parse a prefix as if it were the whole answer.
fn read_capped(reader: impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_OUTPUT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_OUTPUT {
        Err(format!("hyprctl output exceeded {MAX_OUTPUT} bytes"))
    } else {
        Ok(bytes)
    }
}

/// Runs `path` with `args`: no shell, both pipes drained while it runs (a full
/// pipe cannot defeat the deadline), killed and reaped at the deadline.
pub(super) fn command_at(path: &Path, args: &[&str]) -> Result<String, String> {
    let deadline = Instant::now() + TIMEOUT;
    let mut child = Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let stdout = child.stdout.take().ok_or("missing stdout")?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;
    let (tx, rx) = mpsc::channel();
    let out_tx = tx.clone();
    std::thread::spawn(move || {
        let _ = out_tx.send((true, read_capped(stdout)));
    });
    std::thread::spawn(move || {
        let _ = tx.send((false, read_capped(stderr)));
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("hyprctl timed out after 2s".into());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
        }
    };
    let mut out = None;
    let mut err = None;
    for _ in 0..2 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let (is_stdout, bytes) = rx
            .recv_timeout(remaining)
            .map_err(|_| "hyprctl timed out after 2s")?;
        let bytes = bytes?;
        if is_stdout {
            out = Some(bytes);
        } else {
            err = Some(bytes);
        }
    }
    let stdout = out.unwrap_or_default();
    if !status.success() {
        let stderr = err.unwrap_or_default();
        return Err(
            String::from_utf8_lossy(if stderr.is_empty() { &stdout } else { &stderr }).into_owned(),
        );
    }
    String::from_utf8(stdout).map_err(|error| error.to_string())
}

fn command(args: &[&str]) -> Result<String, String> {
    command_at(Path::new(HYPRCTL), args)
}

/// What placement asks of the compositor, so the sequence of calls can be tested
/// against a stand-in that behaves like Hyprland's window handling.
pub(super) trait Compositor {
    /// One `hyprctl` call.
    fn command(&self, args: &[&str]) -> Result<String, String>;
    /// Waits between two calls.
    fn pause(&self, duration: Duration);
}

struct Hyprctl;

impl Compositor for Hyprctl {
    fn command(&self, args: &[&str]) -> Result<String, String> {
        command(args)
    }

    fn pause(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// The gap Omarchy's popovers keep from the bar when the option cannot be read
/// (or is not a sane size).
const DEFAULT_GAP: u32 = 5;
/// The largest gap or border, in pixels, taken as a real setting; a value past it
/// is a misread, not a layout.
const MAX_GAP_PX: f64 = 200.0;

/// Half of Hyprland's `general:gaps_out` (the first value of its CSS form, or the
/// plain int), which is the gap Omarchy's popovers keep from the bar; 5 when the
/// option cannot be read or is outside `0..=200` px.
pub(super) fn parse_gap(json: &str) -> u32 {
    let value = serde_json::from_str::<serde_json::Value>(json).ok();
    let gap = value
        .as_ref()
        .and_then(|v| {
            v.get("css")
                .and_then(|v| v.as_str())
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.parse::<f64>().ok())
                .or_else(|| v.get("int").and_then(|v| v.as_f64()))
        })
        .filter(|n| n.is_finite() && (0.0..=MAX_GAP_PX).contains(n));
    gap.map(|n| (n / 2.0).round() as u32).unwrap_or(DEFAULT_GAP)
}

/// Width of the border Hyprland draws around a window, from
/// `getoption general:border_size -j`; 0 when unreadable or outside `0..=200` px.
pub(super) fn parse_border(json: &str) -> u32 {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| v.get("int")?.as_u64())
        .filter(|n| *n <= MAX_GAP_PX as u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
}

/// Gap plus border; the default gap when the sum does not fit.
pub(super) fn combined_gap(gap: u32, border: u32) -> u32 {
    gap.checked_add(border).unwrap_or(DEFAULT_GAP)
}

/// Distance from the bar and screen edges to the window geometry. Hyprland draws
/// a window's border outside its geometry, so the border is added to keep the
/// same visible gap Omarchy's own popovers have.
pub(super) fn outer_gap(gaps_out: Option<&str>, border_size: Option<&str>) -> u32 {
    combined_gap(
        gaps_out.map_or(DEFAULT_GAP, parse_gap),
        border_size.map_or(0, parse_border),
    )
}

fn gap() -> u32 {
    let gaps_out = command(&["getoption", "general:gaps_out", "-j"]).ok();
    let border_size = command(&["getoption", "general:border_size", "-j"]).ok();
    outer_gap(gaps_out.as_deref(), border_size.as_deref())
}

pub(super) fn parse_anchor(json: &str) -> Result<Anchor, String> {
    let anchor: Anchor = serde_json::from_str(json).map_err(|error| error.to_string())?;
    if anchor.x.is_finite() && anchor.y.is_finite() {
        Ok(anchor)
    } else {
        Err("cursor position is not finite".into())
    }
}

/// Scale Hyprland accepts for a monitor, and the largest size and origin a layout
/// has; past these a response is garbage, not a monitor.
const SCALE_RANGE: std::ops::RangeInclusive<f64> = 0.25..=8.0;
const SIZE_RANGE: std::ops::RangeInclusive<f64> = 1.0..=32768.0;
const ORIGIN_RANGE: std::ops::RangeInclusive<f64> = -65536.0..=65536.0;

/// The whole response or nothing: one monitor that cannot hold a window (no
/// scale, no size, a size or an origin no layout has, a reserved area that is
/// negative or larger than the monitor) rejects the list instead of being
/// filtered out of it.
pub(super) fn parse_monitors(json: &str) -> Result<Vec<Monitor>, String> {
    let monitors: Vec<Monitor> = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let usable = |m: &Monitor| {
        let finite = [m.x, m.y, m.width, m.height, m.scale]
            .iter()
            .all(|n| n.is_finite())
            && m.reserved.iter().all(|n| n.is_finite());
        if !finite
            || !SCALE_RANGE.contains(&m.scale)
            || !SIZE_RANGE.contains(&m.width)
            || !SIZE_RANGE.contains(&m.height)
            || !ORIGIN_RANGE.contains(&m.x)
            || !ORIGIN_RANGE.contains(&m.y)
        {
            return false;
        }
        // `reserved` is in logical pixels: left and right against the logical
        // width, top and bottom against the logical height.
        let [left, top, right, bottom] = m.reserved;
        let (logical_w, logical_h) = (m.width / m.scale, m.height / m.scale);
        (0.0..=logical_w).contains(&left)
            && (0.0..=logical_w).contains(&right)
            && (0.0..=logical_h).contains(&top)
            && (0.0..=logical_h).contains(&bottom)
    };
    if monitors.iter().all(usable) {
        Ok(monitors)
    } else {
        Err("monitor list has an entry without a usable size".into())
    }
}

pub(super) fn parse_clients(json: &str) -> Result<Vec<Client>, String> {
    serde_json::from_str(json).map_err(|error| error.to_string())
}

/// The cursor now, for a click that just arrived. `None` off Hyprland and when
/// `hyprctl` fails (with a warning): the caller falls back to the position the
/// tray protocol reported.
pub(super) fn capture() -> Option<Anchor> {
    if !hyprland() {
        return None;
    }
    match command(&["cursorpos", "-j"]).and_then(|json| parse_anchor(&json)) {
        Ok(anchor) => Some(anchor),
        Err(error) => {
            warn("could not read the cursor position", &error);
            None
        }
    }
}

/// `value` rounded to an `i32`, or `None` when it is not finite or does not fit:
/// an unplaceable frame, never a saturated or wrapped coordinate.
fn checked_i32(value: f64) -> Option<i32> {
    let rounded = value.round();
    (rounded.is_finite() && rounded >= f64::from(i32::MIN) && rounded <= f64::from(i32::MAX))
        .then_some(rounded as i32)
}

/// As [`checked_i32`], for a size.
fn checked_u32(value: f64) -> Option<u32> {
    let rounded = value.round();
    (rounded.is_finite() && rounded >= 0.0 && rounded <= f64::from(u32::MAX))
        .then_some(rounded as u32)
}

/// The monitor holding the anchor, else the first.
fn monitor_for_anchor(anchor: Anchor, monitors: &[Monitor]) -> Option<&Monitor> {
    monitors
        .iter()
        .find(|m| {
            m.scale > 0.0
                && anchor.x >= m.x
                && anchor.x < m.x + m.width / m.scale
                && anchor.y >= m.y
                && anchor.y < m.y + m.height / m.scale
        })
        .or_else(|| monitors.first())
}

/// The popover's frame for a click at `anchor`: centered on it horizontally and
/// held `margin` clear of the monitor's sides and its reserved area; flush with
/// the bar when the anchor is in the upper half of its monitor, with the bottom
/// edge when it is in the lower half. `gap` is `Some` under Omarchy, where it
/// replaces the default margin and is also kept from the bar. The height is
/// what the page asked for, held to what the monitor leaves.
pub(super) fn frame_with_gap(
    anchor: Anchor,
    monitors: &[Monitor],
    width: u32,
    requested: u32,
    gap: Option<u32>,
) -> Option<Frame> {
    let monitor = monitor_for_anchor(anchor, monitors)?;
    if monitor.scale <= 0.0 {
        return None;
    }
    let width = width.max(1);
    let (screen_w, screen_h) = (
        monitor.width / monitor.scale,
        monitor.height / monitor.scale,
    );
    let [left, top, right, bottom] = monitor.reserved;
    let gap_px = gap.map_or(0.0, f64::from);
    let margin = gap.map_or(DEFAULT_MARGIN, f64::from);
    let available = (screen_h - top - bottom - panel::POPOVER_BOTTOM_GAP - gap_px * 2.0).max(0.0);
    let height = checked_u32(f64::from(requested).min(available))?;
    if height == 0 {
        return None;
    }
    let min_x = monitor.x + left + margin;
    let max_x = (monitor.x + screen_w - right - margin - f64::from(width)).max(min_x);
    let x = (anchor.x - f64::from(width) / 2.0).clamp(min_x, max_x);
    let y = if anchor.y < monitor.y + screen_h / 2.0 {
        monitor.y + top + gap_px
    } else {
        monitor.y + screen_h - bottom - f64::from(height) - gap_px
    };
    Some(Frame {
        x: checked_i32(x)?,
        y: checked_i32(y)?,
        width,
        height,
    })
}

/// Which request is the newest. The host advances it on every open, close and
/// resize, so an answer for an older one is recognizably stale.
#[derive(Default)]
pub(super) struct Generation {
    value: AtomicU64,
}

pub(super) fn advance(generation: &Generation) -> u64 {
    generation
        .value
        .fetch_add(1, Ordering::SeqCst)
        .wrapping_add(1)
}

pub(super) fn current(generation: &Generation) -> u64 {
    generation.value.load(Ordering::SeqCst)
}

pub(super) fn is_current(generation: &Generation, expected: u64) -> bool {
    current(generation) == expected
}

/// Runs `dispatch` only while `expected` is still the newest generation. The
/// check and the effect are not one atomic step: see the module docs.
pub(super) fn dispatch_if_current(
    generation: &Generation,
    expected: u64,
    dispatch: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if is_current(generation, expected) {
        dispatch()
    } else {
        Ok(())
    }
}

/// `hyprctl eval` answers `ok` and nothing else on success.
pub(super) fn eval_response(response: &str) -> Result<(), String> {
    if response.trim() == "ok" {
        Ok(())
    } else {
        Err(response.to_owned())
    }
}

fn eval(compositor: &impl Compositor, lua: &str) -> Result<(), String> {
    eval_response(&compositor.command(&["eval", lua])?)
}

/// The match both rules use: class and title are fixed strings of this program.
fn rule_match() -> String {
    format!(r#"{{ class = "^({CLASS})$", title = "^({TITLE})$" }}"#)
}

/// The base rule, registered once at start: the popover floats and is pinned.
/// Omarchy has a helper for it that survives its own rule reloads; plain
/// Hyprland takes `hl.window_rule`.
pub(super) fn prepare_rule() -> String {
    let matcher = rule_match();
    format!(
        r#"if o and o.window then o.window({matcher}, {{ name = "{RULE_NAME}", float = true, pin = true }}) else hl.window_rule({{ name = "{RULE_NAME}", match = {matcher}, float = true, pin = true }}) end"#
    )
}

fn prepare() -> Result<(), String> {
    eval(&Hyprctl, &prepare_rule())
}

/// The monitor name is put in the Lua, so it must be a plain identifier.
fn safe_monitor_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The rule that opens the popover at `frame`: monitor-local coordinates on its
/// monitor. Only numbers and fixed strings reach the Lua; a monitor name that is
/// not a plain identifier is left out, and the size alone is asked for.
pub(super) fn lua_opening_rule(monitor: &Monitor, frame: &Frame) -> String {
    let local = checked_i32(f64::from(frame.x) - monitor.x)
        .zip(checked_i32(f64::from(frame.y) - monitor.y));
    let location = match local {
        Some((x, y)) if safe_monitor_name(&monitor.name) => format!(
            ", monitor = \"{}\", size = {{ {}, {} }}, move = {{ {}, {} }}",
            monitor.name, frame.width, frame.height, x, y,
        ),
        _ => format!(", size = {{ {}, {} }}", frame.width, frame.height),
    };
    let matcher = rule_match();
    format!(
        r#"hl.window_rule({{ name = "{RULE_NAME}", match = {matcher}, float = true, pin = true{location}, animation = "popin 100%" }})"#
    )
}

/// The opening rule, then the same rule with less in it, tried in this order.
///
/// Omarchy's floating workspace mode loads its hyprbars plugin and gives every
/// window on that workspace a titlebar drawn above the window, which sits behind
/// the bar and shows below it as a band. The plugin's `hyprbars:no_bar` effect
/// turns it off, but Hyprland refuses an unknown field, so the first rule asks
/// for it only while the plugin is loaded, and the others leave it out, then drop
/// the animation, in case the field is refused anyway.
pub(super) fn lua_opening_rules(monitor: &Monitor, frame: &Frame) -> [String; 3] {
    let rule = lua_opening_rule(monitor, frame);
    let table = rule
        .strip_prefix("hl.window_rule(")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or_default();
    [
        format!(
            r#"local rule = {table}; if hl.plugin and hl.plugin.hyprbars then rule["hyprbars:no_bar"] = true end; hl.window_rule(rule)"#
        ),
        rule.clone(),
        rule.replace(", animation = \"popin 100%\"", ""),
    ]
}

/// Evaluates each rule until one is accepted; the error is the last one.
pub(super) fn first_accepted(
    rules: &[String],
    mut eval: impl FnMut(&str) -> Result<(), String>,
) -> Result<(), String> {
    let mut last = Err("no popover rule".to_owned());
    for rule in rules {
        last = eval(rule);
        match &last {
            Ok(()) => break,
            Err(error) => warn("could not register the popover rule", error),
        }
    }
    last
}

/// Moves and resizes a mapped window. `address` comes from `hyprctl clients`,
/// and only `0x` plus lowercase hex is let into the Lua.
pub(super) fn lua_for_frame(address: &str, frame: &Frame) -> Option<String> {
    let hex = address.strip_prefix("0x")?;
    if hex.is_empty()
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let target = format!("address:{address}");
    Some(format!(
        "hl.dispatch(hl.dsp.window.float({{ action = \"on\", window = \"{target}\" }})); \
         hl.dispatch(hl.dsp.window.resize({{ x = {}, y = {}, relative = false, window = \"{target}\" }})); \
         hl.dispatch(hl.dsp.window.move({{ x = {}, y = {}, relative = false, window = \"{target}\" }}))",
        frame.width, frame.height, frame.x, frame.y
    ))
}

/// The worker thread's handle. Absent off Hyprland (see [`Positioner::start`]).
pub(super) struct Positioner {
    input: Sender<PlacementRequest>,
}

impl Positioner {
    /// Starts the worker when this is Hyprland and `hyprctl` is where it is
    /// expected; `None` otherwise, and the host keeps its previous behavior.
    /// `reply(generation, height, anchor, opening)` runs on the worker thread;
    /// `height` is `None` when the popover could not be placed.
    pub(super) fn start(
        generation: Arc<Generation>,
        omarchy: bool,
        reply: impl Fn(u64, Option<u32>, Option<Anchor>, bool) + Send + 'static,
    ) -> Option<Self> {
        if !hyprland() || !Path::new(HYPRCTL).is_file() {
            return None;
        }
        let (input, requests) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("ai-usagebar-tray-hypr".into())
            .spawn(move || {
                let ready = match prepare() {
                    Ok(()) => true,
                    Err(error) => {
                        warn("could not register the popover rule", &error);
                        false
                    }
                };
                let mut current_gap = None;
                worker_loop(
                    requests,
                    &generation,
                    || if ready { capture() } else { None },
                    |request| {
                        if !ready {
                            return Ok(None);
                        }
                        if request.is_opening && omarchy {
                            current_gap = Some(gap());
                        }
                        apply(&Hyprctl, request, &generation, current_gap)
                    },
                    reply,
                );
            });
        match spawned {
            Ok(_) => Some(Self { input }),
            Err(error) => {
                warn("could not start the placement worker", &error.to_string());
                None
            }
        }
    }

    /// `Err` when the worker is gone: the caller shows the popover unplaced.
    pub(super) fn send(&self, request: PlacementRequest) -> Result<(), ()> {
        self.input.send(request).map_err(|_| ())
    }
}

/// Serves requests until the sender is dropped. Requests that piled up while one
/// ran are coalesced to the newest, and a request that is no longer the newest
/// generation is dropped without effect and without a reply. A request that could
/// not be placed (no anchor, a failure) is answered with no height: the host then
/// leaves the window as it is instead of resizing it client-side.
pub(super) fn worker_loop(
    requests: Receiver<PlacementRequest>,
    generation: &Generation,
    mut capture_anchor: impl FnMut() -> Option<Anchor>,
    mut process: impl FnMut(PlacementRequest) -> Result<Option<u32>, String>,
    reply: impl Fn(u64, Option<u32>, Option<Anchor>, bool),
) {
    let mut opening_anchor: Option<(u64, Anchor)> = None;
    while let Ok(mut request) = requests.recv() {
        while let Ok(latest) = requests.try_recv() {
            request = latest;
        }
        if !is_current(generation, request.generation) {
            continue;
        }
        if request.anchor.is_none() {
            request.anchor = opening_anchor
                .filter(|(opening, _)| *opening == request.opening)
                .map(|(_, anchor)| anchor)
                .or_else(&mut capture_anchor);
        }
        if let Some(anchor) = request.anchor {
            opening_anchor = Some((request.opening, anchor));
        }
        let height = match process(request) {
            Ok(height) => height,
            Err(error) => {
                warn("could not place the popover", &error);
                None
            }
        };
        if is_current(generation, request.generation) {
            reply(
                request.generation,
                height,
                request.anchor,
                request.is_opening,
            );
        }
    }
}

/// Whether the window is where `frame` puts it, within the rounding fractional
/// scales introduce.
pub(super) fn settled(client: &Client, frame: &Frame) -> bool {
    let close = |a: f64, b: f64| (a - b).abs() <= SETTLE_TOLERANCE;
    close(client.at[0], f64::from(frame.x))
        && close(client.at[1], f64::from(frame.y))
        && close(client.size[0], f64::from(frame.width))
        && close(client.size[1], f64::from(frame.height))
}

/// This process's popover window, when Hyprland has mapped it.
fn own_client(compositor: &impl Compositor) -> Result<Option<Client>, String> {
    let clients = parse_clients(&compositor.command(&["clients", "-j"])?)?;
    Ok(clients
        .into_iter()
        .find(|c| c.pid == std::process::id() && c.title == TITLE))
}

/// Dispatches `lua` and looks at the window until it is at `frame`.
///
/// While GTK still holds the popover at its old fixed size, Hyprland turns a resize
/// down (the move is honored), so one dispatch can leave the window at the old size.
/// Calling that done is what let the host's client-side resize afterwards run
/// Hyprland's resize around the window's center, which lifted a window anchored at
/// the bar behind it. The same absolute move and resize is therefore sent again, up
/// to [`SETTLE_RETRIES`] times, until the window reports the frame; setting the
/// same absolute geometry twice is the same as once. A generation that moved on
/// ends it with no further effect.
fn settle(
    compositor: &impl Compositor,
    generation: &Generation,
    expected: u64,
    frame: &Frame,
    mut lua: String,
) -> Result<Option<u32>, String> {
    for _ in 0..=SETTLE_RETRIES {
        dispatch_if_current(generation, expected, || eval(compositor, &lua))?;
        compositor.pause(SETTLE_PAUSE);
        if !is_current(generation, expected) {
            return Ok(None);
        }
        let Some(client) = own_client(compositor)? else {
            continue;
        };
        if settled(&client, frame) {
            return Ok(Some(frame.height));
        }
        lua = lua_for_frame(&client.address, frame)
            .ok_or("the popover's window address is not a hex address")?;
    }
    Err("the popover did not settle at its frame".into())
}

pub(super) fn apply(
    compositor: &impl Compositor,
    request: PlacementRequest,
    generation: &Generation,
    gap: Option<u32>,
) -> Result<Option<u32>, String> {
    let Some(anchor) = request.anchor else {
        return Ok(None);
    };
    let monitors = parse_monitors(&compositor.command(&["monitors", "-j"])?)?;
    let frame = frame_with_gap(anchor, &monitors, request.width, request.requested, gap)
        .ok_or("no monitor can hold the popover")?;
    if request.is_opening {
        let monitor = monitor_for_anchor(anchor, &monitors).ok_or("no monitor under the cursor")?;
        let rules = lua_opening_rules(monitor, &frame);
        dispatch_if_current(generation, request.generation, || {
            first_accepted(&rules, |rule| eval(compositor, rule))
        })?;
        return Ok(Some(frame.height));
    }
    for attempt in 0..WINDOW_POLLS {
        if !is_current(generation, request.generation) {
            return Ok(None);
        }
        if own_client(compositor)?.is_some() {
            compositor.pause(Duration::from_millis(20));
            if !is_current(generation, request.generation) {
                return Ok(None);
            }
            if let Some(client) = own_client(compositor)? {
                let lua = lua_for_frame(&client.address, &frame)
                    .ok_or("the popover's window address is not a hex address")?;
                if !client.pinned {
                    warn(
                        "the popover is not pinned",
                        &format!("floating: {}", client.floating),
                    );
                }
                return settle(compositor, generation, request.generation, &frame, lua);
            }
        }
        if attempt + 1 < WINDOW_POLLS {
            compositor.pause(Duration::from_millis(20));
        }
    }
    Err("the popover's window did not show up in hyprctl clients".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::os::unix::fs::PermissionsExt;

    const W: u32 = 320;

    fn monitors(json: &str) -> Vec<Monitor> {
        parse_monitors(json).unwrap()
    }

    fn at(x: f64, y: f64) -> Anchor {
        Anchor { x, y }
    }

    fn frame(anchor: Anchor, monitors: &[Monitor], width: u32, requested: u32) -> Option<Frame> {
        frame_with_gap(anchor, monitors, width, requested, None)
    }

    const ONE: &str = r#"[{"name":"DP-1","x":0,"y":0,"width":2560,"height":1440,"scale":1,"reserved":[0,26,0,0]}]"#;
    const TWO: &str = r#"[
        {"name":"eDP-2","x":0,"y":0,"width":1920,"height":1080,"scale":1,"reserved":[0,26,0,0]},
        {"name":"HDMI-A-1","x":1920,"y":0,"width":2560,"height":1440,"scale":1,"reserved":[0,26,0,0]}
    ]"#;

    // --- parsers ------------------------------------------------------------

    #[test]
    fn the_cursor_position_parses_and_a_malformed_one_is_refused() {
        let anchor = parse_anchor(r#"{"x":-120,"y":42}"#).unwrap();
        assert_eq!((anchor.x, anchor.y), (-120.0, 42.0));
        assert!(parse_anchor(r#"{"x":"oops","y":42}"#).is_err());
        assert!(parse_anchor(r#"{"x":1}"#).is_err());
        assert!(parse_anchor("").is_err());
    }

    #[test]
    fn a_monitor_list_parses_whole_or_not_at_all() {
        let list = monitors(
            r#"[{"name":"eDP-2","x":0,"y":0,"width":2560,"height":1600,"scale":1.6,"reserved":[0,26,0,0],"dpmsStatus":true}]"#,
        );
        assert_eq!(list.len(), 1);
        assert!(parse_monitors(r#"[{"x":0}]"#).is_err());
        assert!(parse_monitors("{}").is_err());
        // One entry that cannot hold a window rejects the list: none is dropped.
        for bad in [
            r#"{"x":0,"y":0,"width":1920,"height":1080,"scale":0,"reserved":[0,26,0,0]}"#,
            r#"{"x":0,"y":0,"width":1920,"height":1080,"scale":-1,"reserved":[0,26,0,0]}"#,
            r#"{"x":0,"y":0,"width":0,"height":1080,"scale":1,"reserved":[0,26,0,0]}"#,
        ] {
            let good =
                r#"{"x":0,"y":0,"width":1920,"height":1080,"scale":1,"reserved":[0,26,0,0]}"#;
            assert!(parse_monitors(&format!("[{good},{bad}]")).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_client_list_needs_every_field_placement_reads() {
        let clients = parse_clients(
            r#"[{"pid":123,"title":"AI Usage","address":"0xabcd","floating":true,"pinned":false,"class":"x","at":[1,2],"size":[3,4]}]"#,
        )
        .unwrap();
        assert_eq!(
            (
                clients[0].pid,
                clients[0].title.as_str(),
                clients[0].address.as_str(),
                clients[0].floating,
                clients[0].pinned
            ),
            (123, "AI Usage", "0xabcd", true, false)
        );
        assert!(parse_clients(r#"[{"pid":123,"title":"AI Usage"}]"#).is_err());
        assert!(parse_clients("not json").is_err());
    }

    // --- monitor choice and frame --------------------------------------------

    #[test]
    fn the_monitor_under_the_cursor_is_the_one_chosen() {
        let list = monitors(TWO);
        let first = monitor_for_anchor(at(100.0, 10.0), &list).unwrap();
        let second = monitor_for_anchor(at(2000.0, 10.0), &list).unwrap();
        assert_eq!(
            (first.name.as_str(), second.name.as_str()),
            ("eDP-2", "HDMI-A-1")
        );
        // The cursor on no monitor falls back to the first.
        let off = monitor_for_anchor(at(9000.0, 10.0), &list).unwrap();
        assert_eq!(off.name, "eDP-2");
    }

    #[test]
    fn a_click_on_the_second_monitor_is_framed_on_the_second_monitor() {
        let list = monitors(TWO);
        let placed = frame(at(2000.0, 10.0), &list, W, 360).unwrap();
        // Global x: pulled back to the second monitor's left edge plus margin.
        assert_eq!(
            placed,
            Frame {
                x: 1928,
                y: 26,
                width: W,
                height: 360
            }
        );
        let first = frame(at(100.0, 10.0), &list, W, 360).unwrap();
        assert_eq!(first.x, 8);
    }

    #[test]
    fn the_frame_is_held_inside_the_monitor_at_both_sides() {
        let list = monitors(ONE);
        // Far right: the right edge keeps the margin clear.
        let right = frame(at(2550.0, 10.0), &list, W, 360).unwrap();
        assert_eq!(right.x, 2560 - 8 - W as i32);
        // Far left: the left edge does.
        let left = frame(at(3.0, 10.0), &list, W, 360).unwrap();
        assert_eq!(left.x, 8);
        // Centered when there is room.
        let middle = frame(at(1000.0, 10.0), &list, W, 360).unwrap();
        assert_eq!(middle.x, 840);
    }

    #[test]
    fn the_frame_follows_the_requested_width() {
        let list = monitors(ONE);
        let native = frame(at(2550.0, 10.0), &list, 390, 360).unwrap();
        assert_eq!((native.width, native.x), (390, 2560 - 8 - 390));
        let centered = frame(at(1000.0, 10.0), &list, 390, 360).unwrap();
        assert_eq!(centered.x, 805);
    }

    #[test]
    fn the_reserved_area_and_the_bar_side_are_respected() {
        let reserved = monitors(
            r#"[{"name":"DP-1","x":0,"y":0,"width":1920,"height":1080,"scale":1,"reserved":[40,26,40,30]}]"#,
        );
        let left = frame(at(3.0, 10.0), &reserved, W, 360).unwrap();
        assert_eq!(left.x, 48);
        let right = frame(at(1915.0, 10.0), &reserved, W, 360).unwrap();
        assert_eq!(right.x, 1920 - 40 - 8 - W as i32);
        // Upper half: flush with the bar's lower edge.
        assert_eq!(left.y, 26);
        // Lower half: flush with the reserved bottom.
        let bottom = frame(at(960.0, 1000.0), &reserved, W, 360).unwrap();
        assert_eq!(bottom.y, 1080 - 30 - 360);
    }

    #[test]
    fn a_tall_request_is_held_to_what_the_monitor_leaves() {
        let short = monitors(
            r#"[{"x":0,"y":0,"width":1280,"height":600,"scale":1,"reserved":[0,26,0,0]}]"#,
        );
        let placed = frame(at(600.0, 10.0), &short, W, 720).unwrap();
        // 600 - 26 (bar) - 24 (slack)
        assert_eq!(placed.height, 550);
        // Below the cap the request is kept, as on a 1440p monitor.
        let tall = monitors(ONE);
        assert_eq!(frame(at(600.0, 10.0), &tall, W, 784).unwrap().height, 784);
    }

    #[test]
    fn a_scaled_monitor_is_measured_in_logical_pixels() {
        let scaled = monitors(
            r#"[{"x":0,"y":0,"width":2880,"height":1800,"scale":1.5,"reserved":[0,30,0,0]}]"#,
        );
        // Logical 1920x1200: the right edge is 1920 - 8 - 320.
        let placed = frame(at(1900.0, 5.0), &scaled, W, 900).unwrap();
        assert_eq!(
            placed,
            Frame {
                x: 1592,
                y: 30,
                width: W,
                height: 900
            }
        );
    }

    #[test]
    fn the_lower_half_opens_upward_and_a_monitor_with_no_room_gives_no_frame() {
        let list = monitors(TWO);
        let bottom = frame(at(3000.0, 1300.0), &list, W, 900).unwrap();
        assert_eq!(bottom.y, 1440 - 900);
        let flat =
            monitors(r#"[{"x":0,"y":0,"width":1920,"height":40,"scale":1,"reserved":[0,26,0,0]}]"#);
        assert_eq!(frame(at(10.0, 10.0), &flat, W, 360), None);
        assert_eq!(frame(at(10.0, 10.0), &[], W, 360), None);
    }

    #[test]
    fn omarchy_keeps_the_gap_from_the_bar_and_the_edges() {
        let list = monitors(
            r#"[{"name":"HDMI-A-1","x":1600,"y":0,"width":3225,"height":1800,"scale":1,"reserved":[0,26,0,0]}]"#,
        );
        let placed = frame_with_gap(at(3267.0, 278.0), &list, W, 480, Some(7)).unwrap();
        // Top: reserved bar plus the gap.
        assert_eq!(placed.y, 26 + 7);
        // x: centered on the cursor, inside the margin = gap.
        assert_eq!(placed.x, 3267 - 160);
        let edge = frame_with_gap(at(4800.0, 278.0), &list, W, 480, Some(7)).unwrap();
        assert_eq!(edge.x, 1600 + 3225 - 7 - W as i32);
        let low = frame_with_gap(at(3267.0, 1700.0), &list, W, 480, Some(7)).unwrap();
        assert_eq!(low.y, 1800 - 480 - 7);
    }

    #[test]
    fn the_gap_comes_from_gaps_out_and_the_border_hyprland_draws_outside() {
        assert_eq!(parse_gap(r#"{"css":"10 10 10 10"}"#), 5);
        assert_eq!(parse_gap(r#"{"int":7}"#), 4);
        assert_eq!(parse_gap("bad"), 5);
        assert_eq!(parse_gap(r#"{"css":"-4"}"#), 5);
        assert_eq!(
            parse_border(r#"{"option":"general:border_size","int":2}"#),
            2
        );
        assert_eq!(parse_border(r#"{"int":-1}"#), 0);
        assert_eq!(parse_border("bad"), 0);
        let gaps_out = r#"{"option":"general:gaps_out","css":"10 10 10 10"}"#;
        let border = r#"{"option":"general:border_size","int":2}"#;
        assert_eq!(outer_gap(Some(gaps_out), Some(border)), 7);
        assert_eq!(outer_gap(Some(gaps_out), None), 5);
        assert_eq!(outer_gap(None, Some(border)), 7);
        assert_eq!(outer_gap(None, None), 5);
    }

    #[test]
    fn a_gap_or_border_outside_a_useful_range_falls_back_to_the_default() {
        // 200 px is the last real value; one past it is a misread.
        assert_eq!(parse_gap(r#"{"int":200}"#), 100);
        assert_eq!(parse_gap(r#"{"int":201}"#), 5);
        assert_eq!(parse_gap(r#"{"css":"201 0 0 0"}"#), 5);
        assert_eq!(parse_gap(r#"{"int":0}"#), 0);
        assert_eq!(parse_gap(r#"{"int":1e300}"#), 5);
        assert_eq!(parse_gap(r#"{"int":18446744073709551615}"#), 5);
        assert_eq!(parse_border(r#"{"int":200}"#), 200);
        assert_eq!(parse_border(r#"{"int":201}"#), 0);
        assert_eq!(parse_border(r#"{"int":4294967295}"#), 0);
        assert_eq!(parse_border(r#"{"int":18446744073709551615}"#), 0);
    }

    #[test]
    fn the_gap_and_border_sum_does_not_wrap() {
        assert_eq!(combined_gap(100, 200), 300);
        assert_eq!(combined_gap(u32::MAX, 1), DEFAULT_GAP);
        assert_eq!(combined_gap(1, u32::MAX), DEFAULT_GAP);
        assert_eq!(combined_gap(u32::MAX, 0), u32::MAX);
    }

    fn monitor_json(fields: &str) -> String {
        format!(r#"[{{"name":"DP-1",{fields}}}]"#)
    }

    const GOOD: &str = r#""x":0,"y":0,"width":1920,"height":1080,"scale":1,"reserved":[0,26,0,0]"#;

    #[test]
    fn a_monitor_with_a_scale_size_origin_or_reserved_area_no_layout_has_is_refused() {
        assert!(parse_monitors(&monitor_json(GOOD)).is_ok());
        let bad = [
            // scale outside 0.25..=8
            r#""x":0,"y":0,"width":1920,"height":1080,"scale":0.24,"reserved":[0,26,0,0]"#,
            r#""x":0,"y":0,"width":1920,"height":1080,"scale":8.01,"reserved":[0,26,0,0]"#,
            r#""x":0,"y":0,"width":1920,"height":1080,"scale":1e-300,"reserved":[0,26,0,0]"#,
            // size outside 1..=32768
            r#""x":0,"y":0,"width":0.5,"height":1080,"scale":1,"reserved":[0,26,0,0]"#,
            r#""x":0,"y":0,"width":32769,"height":1080,"scale":1,"reserved":[0,26,0,0]"#,
            r#""x":0,"y":0,"width":1920,"height":0.5,"scale":1,"reserved":[0,26,0,0]"#,
            r#""x":0,"y":0,"width":1920,"height":32769,"scale":1,"reserved":[0,26,0,0]"#,
            // origin outside -65536..=65536
            r#""x":65537,"y":0,"width":1920,"height":1080,"scale":1,"reserved":[0,26,0,0]"#,
            r#""x":-65537,"y":0,"width":1920,"height":1080,"scale":1,"reserved":[0,26,0,0]"#,
            r#""x":0,"y":65537,"width":1920,"height":1080,"scale":1,"reserved":[0,26,0,0]"#,
            r#""x":0,"y":-65537,"width":1920,"height":1080,"scale":1,"reserved":[0,26,0,0]"#,
            r#""x":1e300,"y":0,"width":1920,"height":1080,"scale":1,"reserved":[0,26,0,0]"#,
            // reserved negative, or larger than the logical dimension
            r#""x":0,"y":0,"width":1920,"height":1080,"scale":1,"reserved":[-1,26,0,0]"#,
            r#""x":0,"y":0,"width":1920,"height":1080,"scale":1,"reserved":[0,-1,0,0]"#,
            r#""x":0,"y":0,"width":1920,"height":1080,"scale":1,"reserved":[0,26,-1,0]"#,
            r#""x":0,"y":0,"width":1920,"height":1080,"scale":1,"reserved":[0,26,0,-1]"#,
            r#""x":0,"y":0,"width":1920,"height":1080,"scale":1,"reserved":[1921,26,0,0]"#,
            r#""x":0,"y":0,"width":1920,"height":1080,"scale":1,"reserved":[0,1081,0,0]"#,
            r#""x":0,"y":0,"width":1920,"height":1080,"scale":1,"reserved":[0,26,1921,0]"#,
            r#""x":0,"y":0,"width":1920,"height":1080,"scale":1,"reserved":[0,26,0,1081]"#,
            // reserved is logical: 1000 fits 1920 px at scale 1 but not at scale 2
            r#""x":0,"y":0,"width":1920,"height":1080,"scale":2,"reserved":[0,26,0,541]"#,
            r#""x":0,"y":0,"width":1920,"height":1080,"scale":2,"reserved":[961,26,0,0]"#,
        ];
        for fields in bad {
            assert!(parse_monitors(&monitor_json(fields)).is_err(), "{fields}");
            // One bad entry rejects the list; the good one is not kept alone.
            let both = format!(r#"[{{"name":"A",{GOOD}}},{{"name":"B",{fields}}}]"#);
            assert!(parse_monitors(&both).is_err(), "{fields}");
        }
    }

    #[test]
    fn the_edges_of_every_monitor_limit_are_accepted() {
        for fields in [
            r#""x":-65536,"y":65536,"width":32768,"height":32768,"scale":0.25,"reserved":[0,0,0,0]"#,
            r#""x":65536,"y":-65536,"width":1,"height":1,"scale":8,"reserved":[0,0,0,0]"#,
            r#""x":0,"y":0,"width":1920,"height":1080,"scale":2,"reserved":[960,540,960,540]"#,
        ] {
            assert!(parse_monitors(&monitor_json(fields)).is_ok(), "{fields}");
        }
    }

    #[test]
    fn a_frame_that_does_not_fit_an_i32_is_no_position_not_a_saturated_one() {
        assert_eq!(checked_i32(1.4), Some(1));
        assert_eq!(checked_i32(-1.6), Some(-2));
        assert_eq!(checked_i32(f64::from(i32::MAX)), Some(i32::MAX));
        assert_eq!(checked_i32(f64::from(i32::MAX) + 1.0), None);
        assert_eq!(checked_i32(f64::from(i32::MIN) - 1.0), None);
        assert_eq!(checked_i32(f64::NAN), None);
        assert_eq!(checked_i32(f64::INFINITY), None);
        assert_eq!(checked_u32(0.4), Some(0));
        assert_eq!(checked_u32(-0.6), None);
        assert_eq!(checked_u32(f64::from(u32::MAX) + 1.0), None);
        assert_eq!(checked_u32(f64::NAN), None);
    }

    #[test]
    fn an_anchor_far_outside_the_layout_still_yields_a_clamped_frame() {
        let list = monitors(ONE);
        for far in [1e300, -1e300, f64::from(i32::MAX) * 4.0] {
            let placed = frame(at(far, far), &list, W, 360);
            let placed = placed.expect("a far anchor falls back to the first monitor");
            assert!((0..=2560).contains(&placed.x), "{placed:?}");
        }
    }

    #[test]
    fn a_frame_that_cannot_be_put_in_the_lua_leaves_the_position_out() {
        let list = monitors(ONE);
        let monitor = &list[0];
        let beyond = Frame {
            x: i32::MAX,
            y: 26,
            width: W,
            height: 360,
        };
        // Monitor-local x would be i32::MAX - 0 (fits); a monitor origin far below
        // makes the local offset overflow i32, so only the size is asked for.
        let far = monitors(
            r#"[{"name":"DP-1","x":-65536,"y":0,"width":2560,"height":1440,"scale":1,"reserved":[0,26,0,0]}]"#,
        );
        let rule = lua_opening_rule(&far[0], &beyond);
        assert!(rule.contains("size = { 320, 360 }"), "{rule}");
        assert!(!rule.contains("move ="), "{rule}");
        assert!(!rule.contains("monitor ="), "{rule}");
        // The ordinary case keeps the position.
        let ordinary = Frame {
            x: 100,
            y: 26,
            width: W,
            height: 360,
        };
        assert!(lua_opening_rule(monitor, &ordinary).contains("move = { 100, 26 }"));
    }

    // --- Lua -----------------------------------------------------------------

    #[test]
    fn the_prepare_rule_matches_only_this_popover() {
        let rule = prepare_rule();
        assert!(rule.contains(r#"class = "^(ai-usagebar-tray)$""#));
        assert!(rule.contains(r#"title = "^(AI Usage)$""#));
        assert!(rule.contains("float = true, pin = true"));
        assert!(rule.contains("o.window(") && rule.contains("hl.window_rule("));
    }

    #[test]
    fn the_opening_rule_uses_monitor_local_coordinates() {
        for (name, origin, anchor_x, global_x, local_x) in [
            ("HDMI-A-1", 1600, 3267.0, 3107, 1507),
            ("eDP-2", 0, 100.0, 8, 8),
        ] {
            let list = monitors(&format!(
                r#"[{{"name":"{name}","x":{origin},"y":0,"width":1920,"height":1080,"scale":1,"reserved":[0,26,0,0]}}]"#
            ));
            let placed = frame(at(anchor_x, 10.0), &list, W, 360).unwrap();
            assert_eq!(placed.x, global_x);
            assert_eq!(
                lua_opening_rule(&list[0], &placed),
                format!(
                    r#"hl.window_rule({{ name = "ai-usagebar-tray", match = {{ class = "^(ai-usagebar-tray)$", title = "^(AI Usage)$" }}, float = true, pin = true, monitor = "{name}", size = {{ 320, 360 }}, move = {{ {local_x}, 26 }}, animation = "popin 100%" }})"#
                )
            );
        }
    }

    #[test]
    fn a_hostile_monitor_name_never_reaches_the_lua() {
        for name in [
            "DP-1\"",
            "a b",
            "",
            "DP\n1",
            "éDP",
            "a;b",
            "x\"}); os.execute(\"id",
        ] {
            let json = serde_json::json!([{ "name": name, "x": 0, "y": 0, "width": 1920, "height": 1080, "scale": 1, "reserved": [0, 26, 0, 0] }]);
            let list = monitors(&json.to_string());
            let placed = frame(at(100.0, 10.0), &list, W, 360).unwrap();
            let rules = lua_opening_rules(&list[0], &placed);
            for rule in &rules {
                assert!(!rule.contains("monitor ="), "{name:?}: {rule}");
                assert!(!rule.contains("os.execute"), "{name:?}: {rule}");
                assert!(!rule.contains("move ="), "{name:?}: {rule}");
            }
            assert_eq!(
                lua_opening_rule(&list[0], &placed),
                r#"hl.window_rule({ name = "ai-usagebar-tray", match = { class = "^(ai-usagebar-tray)$", title = "^(AI Usage)$" }, float = true, pin = true, size = { 320, 360 }, animation = "popin 100%" })"#,
                "{name:?}"
            );
        }
    }

    #[test]
    fn the_rule_is_retried_with_less_in_it() {
        let list = monitors(ONE);
        let placed = frame(at(100.0, 10.0), &list, W, 360).unwrap();
        let [first, second, third] = lua_opening_rules(&list[0], &placed);
        assert!(first.starts_with("local rule = { name = "));
        assert!(first.contains(r#"rule["hyprbars:no_bar"] = true"#));
        assert!(first.ends_with("hl.window_rule(rule)"));
        assert!(second.starts_with("hl.window_rule({") && second.contains("popin 100%"));
        assert!(!third.contains("animation"));
        // The first accepted rule ends the walk; a refusal moves to the next.
        let tried = RefCell::new(Vec::new());
        let result = first_accepted(&[first.clone(), second.clone(), third], |rule| {
            tried.borrow_mut().push(rule.to_owned());
            if tried.borrow().len() < 2 {
                Err("refused".into())
            } else {
                Ok(())
            }
        });
        assert!(result.is_ok());
        assert_eq!(*tried.borrow(), [first, second]);
        assert_eq!(
            first_accepted(&[], |_| Ok(())),
            Err("no popover rule".into())
        );
        let all_refused = first_accepted(&["a".into(), "b".into()], |rule| Err(rule.to_owned()));
        assert_eq!(all_refused, Err("b".into()));
    }

    #[test]
    fn the_move_lua_targets_only_valid_lowercase_hex_addresses() {
        let placed = Frame {
            x: 10,
            y: 26,
            width: 390,
            height: 500,
        };
        assert_eq!(
            lua_for_frame("0x12ab", &placed).as_deref(),
            Some(
                "hl.dispatch(hl.dsp.window.float({ action = \"on\", window = \"address:0x12ab\" })); hl.dispatch(hl.dsp.window.resize({ x = 390, y = 500, relative = false, window = \"address:0x12ab\" })); hl.dispatch(hl.dsp.window.move({ x = 10, y = 26, relative = false, window = \"address:0x12ab\" }))"
            )
        );
        for bad in [
            "",
            "0x",
            "12ab",
            "0X12ab",
            "0x12AB",
            "0x12 ab",
            "0x12\"ab",
            "0x1;2",
            "0xg",
            "0x12ab\n",
            "0x\") os.execute(\"id",
        ] {
            assert!(lua_for_frame(bad, &placed).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn eval_accepts_only_ok_with_whitespace() {
        assert!(eval_response(" \nok \t").is_ok());
        for response in ["", "error", "okay", "ok ok", "not ok"] {
            assert!(eval_response(response).is_err(), "{response:?}");
        }
    }

    // --- generation ----------------------------------------------------------

    #[test]
    fn a_newer_generation_cancels_the_effect_of_an_older_one() {
        let generation = Generation::default();
        let first = advance(&generation);
        assert!(is_current(&generation, first));
        let second = advance(&generation);
        let mut effects = Vec::new();
        dispatch_if_current(&generation, first, || {
            effects.push("first");
            Ok(())
        })
        .unwrap();
        dispatch_if_current(&generation, second, || {
            effects.push("second");
            Ok(())
        })
        .unwrap();
        assert_eq!(effects, ["second"]);
        // A hide between the read and the effect invalidates that opening.
        advance(&generation);
        dispatch_if_current(&generation, second, || {
            effects.push("hidden");
            Ok(())
        })
        .unwrap();
        assert_eq!(effects, ["second"]);
        assert!(!is_current(&generation, first) && !is_current(&generation, second));
    }

    #[test]
    fn a_hide_that_lands_between_the_check_and_the_dispatch_stops_the_dispatch() {
        for reopen in [true, false] {
            let generation = Generation::default();
            let first = advance(&generation);
            let (ready_tx, ready_rx) = mpsc::channel();
            let (resume_tx, resume_rx) = mpsc::channel();
            let (effect_tx, effect_rx) = mpsc::channel();
            std::thread::scope(|scope| {
                let generation_ref = &generation;
                let worker_effect_tx = effect_tx.clone();
                let worker = scope.spawn(move || {
                    // The worker has read the generation and is about to dispatch.
                    let _ = ready_tx.send(is_current(generation_ref, first));
                    let _ = resume_rx.recv_timeout(Duration::from_secs(2));
                    dispatch_if_current(generation_ref, first, || {
                        let _ = worker_effect_tx.send("stale");
                        Ok(())
                    })
                    .unwrap();
                });
                assert!(ready_rx.recv_timeout(Duration::from_secs(2)).unwrap());
                let next = advance(&generation);
                resume_tx.send(()).unwrap();
                worker.join().unwrap();
                if reopen {
                    dispatch_if_current(&generation, next, || {
                        let _ = effect_tx.send("current");
                        Ok(())
                    })
                    .unwrap();
                }
            });
            drop(effect_tx);
            let effects: Vec<_> = effect_rx.into_iter().collect();
            assert_eq!(effects, if reopen { vec!["current"] } else { vec![] });
        }
    }

    #[test]
    fn advancing_does_not_wait_for_a_dispatch_in_flight() {
        let generation = Arc::new(Generation::default());
        let first = advance(&generation);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker_generation = Arc::clone(&generation);
        let worker = std::thread::spawn(move || {
            dispatch_if_current(&worker_generation, first, || {
                let _ = entered_tx.send(());
                let _ = release_rx.recv_timeout(Duration::from_secs(2));
                Ok(())
            })
            .unwrap();
        });
        let entered = entered_rx.recv_timeout(Duration::from_secs(2));
        let (done_tx, done_rx) = mpsc::channel();
        let advancing_generation = Arc::clone(&generation);
        let advancing = std::thread::spawn(move || {
            let _ = done_tx.send(advance(&advancing_generation));
        });
        let advanced_before_release = done_rx.recv_timeout(Duration::from_millis(250));
        let _ = release_tx.send(());
        worker.join().unwrap();
        advancing.join().unwrap();
        assert!(entered.is_ok());
        assert_eq!(advanced_before_release.unwrap(), first + 1);
    }

    #[test]
    fn the_worker_drops_stale_requests_and_coalesces_the_rest() {
        let generation = Arc::new(Generation::default());
        let (input, requests) = mpsc::channel();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (processed_tx, processed_rx) = mpsc::channel();
        let (effect_tx, effect_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let worker_generation = Arc::clone(&generation);
        let worker = std::thread::spawn(move || {
            worker_loop(
                requests,
                &worker_generation,
                || None,
                |request| {
                    let _ = processed_tx.send(request.generation);
                    if request.generation == 1 {
                        let _ = entered_tx.send(());
                        let _ = release_rx.recv_timeout(Duration::from_secs(2));
                    }
                    dispatch_if_current(&worker_generation, request.generation, || {
                        let _ = effect_tx.send(request.generation);
                        Ok(())
                    })?;
                    Ok(Some(request.requested))
                },
                |generation, height, _, _| {
                    let _ = reply_tx.send((generation, height));
                },
            );
        });
        let first = advance(&generation);
        input.send(click_request(first, None, W, 200)).unwrap();
        let entered = entered_rx.recv_timeout(Duration::from_secs(2));
        let second = advance(&generation);
        input.send(click_request(second, None, W, 300)).unwrap();
        let third = advance(&generation);
        input.send(click_request(third, None, W, 400)).unwrap();
        let _ = release_tx.send(());
        drop(input);
        worker.join().unwrap();
        assert!(entered.is_ok());
        // The first ran (it was in flight); the second was coalesced away.
        assert_eq!(processed_rx.into_iter().collect::<Vec<_>>(), [first, third]);
        // Its effect was skipped, because the generation had moved on.
        assert_eq!(effect_rx.into_iter().collect::<Vec<_>>(), [third]);
        assert_eq!(
            reply_rx.into_iter().collect::<Vec<_>>(),
            [(third, Some(400))]
        );
    }

    #[test]
    fn a_reply_is_applied_only_for_the_newest_generation() {
        let list = monitors(
            r#"[{"x":0,"y":0,"width":1280,"height":600,"scale":1,"reserved":[0,26,0,0]}]"#,
        );
        let generation = Generation::default();
        let current_gen = advance(&generation);
        let (input, requests) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        input
            .send(click_request(current_gen, Some(at(600.0, 10.0)), W, 720))
            .unwrap();
        drop(input);
        worker_loop(
            requests,
            &generation,
            || None,
            |request| {
                Ok(frame(
                    request.anchor.unwrap(),
                    &list,
                    request.width,
                    request.requested,
                )
                .map(|f| f.height))
            },
            |generation, height, _, _| {
                let _ = reply_tx.send((generation, height));
            },
        );
        let (placed, height) = reply_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!((placed, height), (current_gen, Some(550)));
        assert_eq!(applied_height(current_gen + 1, placed, height), None);
        assert_eq!(applied_height(current_gen, placed, height), Some(Some(550)));
    }

    #[test]
    fn a_resize_keeps_the_opening_anchor_after_the_cursor_moves() {
        let generation = Generation::default();
        let list = monitors(
            r#"[{"x":0,"y":0,"width":1920,"height":1080,"scale":1,"reserved":[0,26,0,0]}]"#,
        );
        let pointer = Cell::new(at(100.0, 10.0));
        let captures = Cell::new(0);
        let moves = RefCell::new(Vec::new());
        let mut stored: Option<Anchor> = None;
        let mut opening = 0;
        for height in [360, 500] {
            let current_gen = advance(&generation);
            let (tx, rx) = mpsc::channel();
            let request = if opening == 0 {
                opening = current_gen;
                click_request(current_gen, None, W, height)
            } else {
                resize_request(current_gen, opening, stored, W, height)
            };
            tx.send(request).unwrap();
            drop(tx);
            let (reply_tx, reply_rx) = mpsc::channel();
            worker_loop(
                rx,
                &generation,
                || {
                    captures.set(captures.get() + 1);
                    Some(pointer.get())
                },
                |request| {
                    let f = frame(request.anchor.unwrap(), &list, W, request.requested).unwrap();
                    moves.borrow_mut().push((f.x, f.y));
                    Ok(Some(f.height))
                },
                |_, _, anchor, _| {
                    let _ = reply_tx.send(anchor);
                },
            );
            stored = stored.or(reply_rx.recv().unwrap());
            assert_eq!(stored, Some(at(100.0, 10.0)));
            pointer.set(at(250.0, 10.0));
        }
        assert_eq!(*moves.borrow(), [(8, 26), (8, 26)]);
        assert_eq!(
            captures.get(),
            1,
            "the resize must not read the cursor again"
        );
    }

    #[test]
    fn a_new_opening_captures_the_cursor_again() {
        let generation = Generation::default();
        let count = Cell::new(0);
        let seen = RefCell::new(Vec::new());
        for _ in 0..2 {
            let opening = advance(&generation);
            let (tx, rx) = mpsc::channel();
            tx.send(click_request(opening, None, W, 360)).unwrap();
            drop(tx);
            worker_loop(
                rx,
                &generation,
                || {
                    count.set(count.get() + 1);
                    Some(at(100.0 * f64::from(count.get()), 10.0))
                },
                |_| Ok(None),
                |_, _, anchor, _| seen.borrow_mut().push(anchor),
            );
        }
        assert_eq!(
            *seen.borrow(),
            [Some(at(100.0, 10.0)), Some(at(200.0, 10.0))]
        );
    }

    #[test]
    fn a_failed_placement_replies_with_no_height() {
        let generation = Generation::default();
        let current_gen = advance(&generation);
        let (tx, rx) = mpsc::channel();
        tx.send(click_request(current_gen, Some(at(1.0, 1.0)), W, 480))
            .unwrap();
        drop(tx);
        let seen = RefCell::new(Vec::new());
        worker_loop(
            rx,
            &generation,
            || None,
            |_| Err("hyprctl timed out after 2s".into()),
            |generation, height, _, opening| seen.borrow_mut().push((generation, height, opening)),
        );
        assert_eq!(*seen.borrow(), [(current_gen, None, true)]);
    }

    // --- resizing a window that is on screen --------------------------------

    /// Stands in for Hyprland's handling of the one popover window. The popover's GTK
    /// window is fixed (min = max) when a resize is dispatched, and Hyprland turns a
    /// resize down until GTK has let those hints go: `locked_polls` is how many
    /// `clients` reads that takes. A move is always honored. Nothing here is a
    /// timing guess: the lock is counted in calls, so the order is the test.
    struct FakeHypr {
        monitors: String,
        window: RefCell<([f64; 2], [f64; 2])>,
        locked_polls: Cell<u32>,
        evals: RefCell<Vec<String>>,
        on_pause: RefCell<Box<dyn FnMut()>>,
    }

    impl FakeHypr {
        fn new(monitors: &str, at: [f64; 2], size: [f64; 2], locked_polls: u32) -> Self {
            Self {
                monitors: monitors.to_owned(),
                window: RefCell::new((at, size)),
                locked_polls: Cell::new(locked_polls),
                evals: RefCell::new(Vec::new()),
                on_pause: RefCell::new(Box::new(|| {})),
            }
        }

        fn window(&self) -> ([f64; 2], [f64; 2]) {
            *self.window.borrow()
        }

        /// The two numbers after `x = ` and `y = ` that follow `call` in `lua`.
        fn pair(lua: &str, call: &str) -> [f64; 2] {
            let rest = &lua[lua.find(call).unwrap() + call.len()..];
            let number = |key: &str| -> f64 {
                let from = rest.find(key).unwrap() + key.len();
                let text: String = rest[from..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '-')
                    .collect();
                text.parse().unwrap()
            };
            [number("x = "), number("y = ")]
        }
    }

    impl Compositor for FakeHypr {
        fn command(&self, args: &[&str]) -> Result<String, String> {
            match args {
                ["monitors", "-j"] => Ok(self.monitors.clone()),
                ["clients", "-j"] => {
                    self.locked_polls
                        .set(self.locked_polls.get().saturating_sub(1));
                    let (at, size) = self.window();
                    Ok(serde_json::json!([{
                        "address": "0xabc", "pid": std::process::id(), "title": TITLE,
                        "floating": true, "pinned": true, "at": at, "size": size,
                    }])
                    .to_string())
                }
                ["eval", lua] => {
                    self.evals.borrow_mut().push((*lua).to_owned());
                    let mut window = self.window.borrow_mut();
                    if lua.contains("window.resize(") && self.locked_polls.get() == 0 {
                        window.1 = Self::pair(lua, "window.resize(");
                    }
                    if lua.contains("window.move(") {
                        window.0 = Self::pair(lua, "window.move(");
                    }
                    Ok("ok".into())
                }
                other => Err(format!("unexpected hyprctl call {other:?}")),
            }
        }

        fn pause(&self, _: Duration) {
            (self.on_pause.borrow_mut())();
        }
    }

    const HDMI: &str = r#"[{"name":"HDMI-A-1","x":1600,"y":0,"width":3440,"height":1440,"scale":1.0666667,"reserved":[0,26,0,0]}]"#;

    /// Opens at height `from` and resizes to `to`, with the cursor in the upper
    /// (`top`) or lower half. The window starts where the opening put it.
    fn resized(
        top: bool,
        from: u32,
        to: u32,
        locked_polls: u32,
    ) -> (FakeHypr, Result<Option<u32>, String>) {
        let click = if top {
            at(3000.0, 13.0)
        } else {
            at(3000.0, 1100.0)
        };
        let list = monitors(HDMI);
        let opened = frame_with_gap(click, &list, W, from, Some(7)).unwrap();
        let fake = FakeHypr::new(
            HDMI,
            [f64::from(opened.x), f64::from(opened.y)],
            [f64::from(opened.width), f64::from(opened.height)],
            locked_polls,
        );
        let generation = Generation::default();
        let current_gen = advance(&generation);
        let request = resize_request(current_gen, 1, Some(click), W, to);
        let result = apply(&fake, request, &generation, Some(7));
        (fake, result)
    }

    #[test]
    fn a_resize_keeps_the_bar_edge_fixed_whether_the_window_grows_or_shrinks() {
        // (anchored at the top?, height at opening, height asked for)
        for (top, from, to) in [
            (true, 600, 900),
            (true, 900, 600),
            (false, 600, 900),
            (false, 900, 600),
            // Down to the smallest popover (panel::MIN_POPOVER_HEIGHT), below the
            // window's initial height: the detail screens of a provider.
            (true, 420, 360),
            (true, 900, 360),
            (false, 420, 360),
            (false, 900, 360),
        ] {
            // The hints are already released: the resize is honored at once.
            let (fake, result) = resized(top, from, to, 0);
            assert_eq!(result, Ok(Some(to)), "top={top} {from}->{to}");
            let (at_xy, size) = fake.window();
            assert_eq!(size, [320.0, f64::from(to)], "top={top} {from}->{to}");
            let edge = if top { at_xy[1] } else { at_xy[1] + size[1] };
            // Bar (26) + gap (7) at the top; the monitor's bottom (1350) - gap at the bottom.
            assert_eq!(
                edge,
                if top { 33.0 } else { 1343.0 },
                "top={top} {from}->{to}"
            );
            assert_eq!(
                at_xy[0], 2840.0,
                "x must not change: top={top} {from}->{to}"
            );
        }
    }

    #[test]
    fn a_resize_that_never_holds_stops_after_its_retries_and_reports_it() {
        // 1000 locked polls: Hyprland never takes the resize.
        let (fake, result) = resized(true, 600, 900, 1000);
        assert_eq!(
            result,
            Err("the popover did not settle at its frame".into())
        );
        let resizes = fake
            .evals
            .borrow()
            .iter()
            .filter(|lua| lua.contains("window.resize("))
            .count();
        assert_eq!(resizes, 1 + SETTLE_RETRIES as usize);
    }

    #[test]
    fn a_newer_request_stops_the_retries_without_another_dispatch() {
        let list = monitors(HDMI);
        let click = at(3000.0, 13.0);
        let opened = frame_with_gap(click, &list, W, 600, Some(7)).unwrap();
        let fake = FakeHypr::new(
            HDMI,
            [f64::from(opened.x), f64::from(opened.y)],
            [f64::from(opened.width), f64::from(opened.height)],
            1000,
        );
        let generation = Arc::new(Generation::default());
        let current_gen = advance(&generation);
        // A new opening or close lands while the first dispatch is being checked.
        let moved = Arc::clone(&generation);
        // Pause 1 is the settle before the dispatch, pause 2 the look after the first one.
        let pauses = Cell::new(0);
        *fake.on_pause.borrow_mut() = Box::new(move || {
            pauses.set(pauses.get() + 1);
            if pauses.get() == 2 {
                advance(&moved);
            }
        });
        let request = resize_request(current_gen, 1, Some(click), W, 900);
        assert_eq!(apply(&fake, request, &generation, Some(7)), Ok(None));
        assert_eq!(
            fake.evals.borrow().len(),
            1,
            "no dispatch after the generation moved"
        );
    }

    #[test]
    fn a_window_within_rounding_of_its_frame_counts_as_settled() {
        let placed = Frame {
            x: 100,
            y: 33,
            width: 320,
            height: 900,
        };
        let client = |at: [f64; 2], size: [f64; 2]| Client {
            address: "0xabc".into(),
            pid: 1,
            title: TITLE.into(),
            floating: true,
            pinned: true,
            at,
            size,
        };
        assert!(settled(&client([100.0, 33.0], [320.0, 900.0]), &placed));
        assert!(settled(&client([100.0, 33.0], [320.0, 901.0]), &placed));
        assert!(!settled(&client([100.0, 33.0], [320.0, 600.0]), &placed));
        // The top edge raised behind the bar is not settled.
        assert!(!settled(&client([100.0, -120.0], [320.0, 900.0]), &placed));
        assert!(!settled(&client([160.0, 33.0], [320.0, 900.0]), &placed));
    }

    #[test]
    fn a_resize_the_compositor_turns_down_is_dispatched_again_until_it_holds() {
        // GTK has not let go of the old fixed size when the first resize arrives, so Hyprland
        // refuses it. Reporting success then leaves the window at the old size, and the host
        // fixing it client-side afterwards makes Hyprland resize around the center: the top
        // edge rises behind the bar.
        for (top, from, to) in [
            (true, 600, 900),
            (true, 900, 600),
            (false, 600, 900),
            (false, 900, 600),
        ] {
            let (fake, result) = resized(top, from, to, 3);
            let (at_xy, size) = fake.window();
            assert_eq!(
                size,
                [320.0, f64::from(to)],
                "top={top} {from}->{to}: {result:?}"
            );
            let edge = if top { at_xy[1] } else { at_xy[1] + size[1] };
            assert_eq!(
                edge,
                if top { 33.0 } else { 1343.0 },
                "top={top} {from}->{to}"
            );
            assert_eq!(result, Ok(Some(to)));
        }
    }

    // --- hyprctl runner -------------------------------------------------------

    fn script(dir: &tempfile::TempDir, body: &str) -> std::path::PathBuf {
        let path = dir.path().join("fake hyprctl");
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn a_hung_hyprctl_is_killed_and_reaped_at_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid file");
        let fake = script(
            &dir,
            "#!/bin/sh\nprintf '%s' \"$$\" > \"$1\"\nexec sleep 5\n",
        );
        let start = Instant::now();
        let result = command_at(&fake, &[pid_file.to_str().unwrap()]);
        let elapsed = start.elapsed();
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let alive = Path::new("/proc").join(pid.trim()).exists();
        assert_eq!(result.unwrap_err(), "hyprctl timed out after 2s");
        assert!(
            elapsed <= Duration::from_millis(2500),
            "elapsed={elapsed:?}"
        );
        assert!(!alive, "child {pid} outlived the timeout");
    }

    #[test]
    fn the_runner_returns_stdout_and_turns_a_failure_into_its_message() {
        let dir = tempfile::tempdir().unwrap();
        let ok = script(&dir, "#!/bin/sh\nprintf '%s' \"$1 $2\"\n");
        // Arguments with spaces and quotes arrive untouched: no shell.
        assert_eq!(
            command_at(&ok, &["a b", "c\"d;$(id)"]).unwrap(),
            "a b c\"d;$(id)"
        );
        let failing = script(&dir, "#!/bin/sh\nprintf 'boom' >&2\nexit 3\n");
        assert_eq!(command_at(&failing, &[]).unwrap_err(), "boom");
        assert!(command_at(&dir.path().join("missing"), &[]).is_err());
    }

    // Both streams are read to MAX_OUTPUT + 1: an output past the cap is an error,
    // never a prefix parsed as the whole answer. The scripts print a few bytes more
    // than the cap, which fits the pipe once the reader has stopped, so they still
    // exit 0 and the old reader (stopping at the cap) would have returned Ok.
    fn over_the_cap_script(
        dir: &tempfile::TempDir,
        stream: &str,
        bytes: u64,
    ) -> std::path::PathBuf {
        let redirect = if stream == "stderr" { " >&2" } else { "" };
        script(
            dir,
            &format!("#!/bin/sh\nhead -c {bytes} /dev/zero{redirect}\nexit 0\n"),
        )
    }

    #[test]
    fn stdout_past_the_cap_is_an_error_not_a_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let fake = over_the_cap_script(&dir, "stdout", MAX_OUTPUT + 16);
        let error = command_at(&fake, &[]).unwrap_err();
        assert!(error.starts_with("hyprctl output exceeded"), "{error}");
    }

    #[test]
    fn stderr_past_the_cap_is_an_error_even_when_the_command_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let fake = over_the_cap_script(&dir, "stderr", MAX_OUTPUT + 16);
        let error = command_at(&fake, &[]).unwrap_err();
        assert!(error.starts_with("hyprctl output exceeded"), "{error}");
    }

    #[test]
    fn an_output_exactly_at_the_cap_is_still_read_whole() {
        let dir = tempfile::tempdir().unwrap();
        let fake = over_the_cap_script(&dir, "stdout", MAX_OUTPUT);
        let output = command_at(&fake, &[]).unwrap();
        assert_eq!(output.len() as u64, MAX_OUTPUT);
    }

    #[test]
    fn an_output_past_the_cap_is_refused_before_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let fake = over_the_cap_script(&dir, "stdout", MAX_OUTPUT + 16);
        let start = Instant::now();
        let _ = command_at(&fake, &[]);
        assert!(start.elapsed() < Duration::from_millis(1500));
    }

    // --- Omarchy detection ----------------------------------------------------

    #[test]
    fn omarchy_is_detected_from_the_override_or_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let override_dir = dir.path().join("override");
        let default_dir = dir.path().join("default");
        for root in [&override_dir, &default_dir] {
            std::fs::create_dir_all(root.join("shell")).unwrap();
        }
        assert!(!omarchy_detected(Some(&override_dir), &default_dir));
        std::fs::write(override_dir.join("shell/shell.qml"), "").unwrap();
        assert!(omarchy_detected(Some(&override_dir), &default_dir));
        assert!(!omarchy_detected(None, &default_dir));
        assert!(!omarchy_detected(Some(Path::new("")), &default_dir));
        std::fs::write(default_dir.join("shell/shell.qml"), "").unwrap();
        assert!(omarchy_detected(None, &default_dir));
    }
}
