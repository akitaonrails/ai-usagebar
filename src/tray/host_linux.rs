//! StatusNotifierItem + WebKitGTK popover. Linux-only, behind the opt-in
//! `linux-tray` feature.
//!
//! Same shell as the Windows host (`host.rs`): a `tao` event loop, a `tray-icon`
//! icon, a `wry` WebView serving the shared React popover, and a worker thread
//! that refreshes the report. What differs on Linux:
//!
//! - the icon is a StatusNotifierItem (`tray-icon`'s `ksni` backend). The host
//!   gives a click *position*, never the icon's rectangle, and draws the
//!   right-click menu itself from the snapshot of the `muda` menu;
//! - there is no blur/glass material, so the `Native` style renders as `Classic`;
//! - a Wayland compositor may ignore `set_outer_position` and `always_on_top`:
//!   the popover is then placed by the compositor. That is accepted, not worked
//!   around;
//! - the global shortcut is not registered: `set-shortcut` leaves it
//!   unavailable through the same outcome/fact the other hosts use when the
//!   registration fails;
//! - clipboard text and PNG go through GTK, on the main thread.
//!
//! Threads: the event-loop thread owns every window, webview and menu object.
//! The worker, the `tray-icon` handlers (the ksni thread) and the
//! device-login callbacks only `send_event` through the `EventLoopProxy`.

use std::borrow::Cow;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use fs2::FileExt;
use gdk::prelude::MonitorExt;
use gdk_pixbuf::prelude::PixbufLoaderExt;
use serde_json::Value;
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::monitor::MonitorHandle;
use tao::platform::unix::{MonitorHandleExtUnix, WindowBuilderExtUnix, WindowExtUnix};
use tao::window::{Window, WindowBuilder};
use tray_icon::menu::{Menu, MenuEvent};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use wry::http::{Request, Response, StatusCode, header::CONTENT_TYPE};
use wry::{NewWindowResponse, WebContext, WebView, WebViewBuilder, WebViewBuilderExtUnix};

use super::browse;
use super::hotkey;
use super::icon::{Ink, Severity, apply_ink, tray_icon_rgba};
use super::ipc_origin::{self, Navigation, Os};
use super::options_menu::{self, OptionsAction, OptionsLabels};
use super::panel;
use super::payload::{
    HostFacts, SharedFacts, facts_snapshot, host_payload, with_facts, worst_severity, wrap_report,
};
use super::placement::{self, Area, Insets};
use super::strip::{Stars, UsageReading, content_from_payload, parse_strip_ipc, tooltip};
use super::style::PopoverStyle;
use super::updates::Updates;
use super::{RELAUNCH_ENV, now_ms, profile, startup};
use crate::config::{Config, UpdateMode};

// Emitted by `windows/popover` (`npm run build` / `build.rs` with `linux-tray`).
const INDEX_HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/popover/index.html"));
const POPOVER_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/popover/popover.css"));
const POPOVER_JS: &str = include_str!(concat!(env!("OUT_DIR"), "/popover/popover.js"));

/// The popover's one origin: loaded through the `aiub` custom protocol, the
/// same URL the macOS host loads.
const POPOVER_URL: &str = "aiub://localhost/index.html";
/// Window title, also what the smoke test and compositor rules match on.
const WINDOW_TITLE: &str = "AI Usage";
/// Gap between the popover and the monitor edge or the click, physical px.
const POPOVER_MARGIN: i32 = 8;
/// Where the popover goes when no monitor can be resolved at all.
const DEFAULT_POSITION: (i32, i32) = (80, 80);
/// The page ignores clicks this long after opening, so a stray mouse-up cannot hit the ⋮.
const CLICK_LOCK_MS: u64 = 400;
/// A `Focused(false)` this soon after the popover opened is the one our own
/// click on the icon (or the show/focus call) bounces through the compositor.
const BLUR_DEBOUNCE: Duration = Duration::from_millis(350);
/// A click on the icon this soon after a blur closed the popover is the same
/// click: the press took the focus away first, and the release must not reopen.
const REOPEN_DEBOUNCE: Duration = Duration::from_millis(350);
/// How long a relaunched process keeps retrying the lock.
const RELAUNCH_WAIT: Duration = Duration::from_secs(10);
/// Raster side asked of the icon set; the panel scales it to its own slot.
const ICON_SIDE: u32 = 32;
/// Linux panels are dark more often than not (Waybar's default, Omarchy), and
/// there is no portable way to ask the panel; a fixed white glyph it is.
const ICON_INK: Ink = Ink::White;

/// What the page said it is told on `resize`; tao is the only place a theme
/// reaches the window.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Theme {
    Light,
    Dark,
}

impl Theme {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    fn background(self) -> (u8, u8, u8, u8) {
        match self {
            Self::Light => panel::LIGHT_BACKGROUND,
            Self::Dark => panel::DARK_BACKGROUND,
        }
    }

    fn window_theme(self) -> tao::window::Theme {
        match self {
            Self::Light => tao::window::Theme::Light,
            Self::Dark => tao::window::Theme::Dark,
        }
    }
}

enum UserEvent {
    Tray(TrayIconEvent),
    Menu(MenuEvent),
    Ipc(String),
    Report(Value),
    /// The shared facts changed (shortcut, update state); re-stamp the payload.
    Facts,
    /// An update asked for a restart. Linux never self-updates (`installable`
    /// refuses), so this is only the shared worker's callback being satisfied.
    Restart(Option<PathBuf>),
}

enum WorkerCmd {
    Refresh,
    /// Re-read the provider catalog alone, without a report.
    Providers,
    Detect,
    CheckUpdate {
        manual: bool,
    },
    InstallUpdate,
    SnoozeUpdate,
    SetUpdates(UpdateMode),
    Shutdown,
}

struct TrayState {
    window: Window,
    webview: Option<WebView>,
    /// wry ties the WebKit data directory to the context, so it outlives the webview.
    _web_context: WebContext,
    tray: TrayIcon,
    /// Labels for the right-click Options menu; the popover refreshes them by
    /// language with the `menu-labels` IPC.
    menu_labels: OptionsLabels,
    /// A right-click screen choice made before the popover's `ready`: the hook
    /// only exists after it, so the choice waits for `ready` to run.
    pending_menu_action: Option<&'static str>,
    /// One `Menu` for the whole run; the ksni snapshot follows it, so a refill
    /// reaches the panel's menu without replacing the object.
    context_menu: Menu,
    worker: mpsc::Sender<WorkerCmd>,
    proxy: EventLoopProxy<UserEvent>,
    payload: Value,
    stars: Stars,
    strip_order: Vec<String>,
    usage_reading: UsageReading,
    js_ready: bool,
    popover_open: bool,
    /// When the popover last opened, for [`blur_closes`].
    last_shown: Option<Instant>,
    /// When a blur last closed the popover, for [`click_reopens`].
    last_blur_hide: Option<Instant>,
    /// The last usable click position, reused when a menu entry opens the popover.
    last_anchor: Option<(i32, i32)>,
    theme: Theme,
    style: PopoverStyle,
    /// Last valid CSS/logical height reported by the page.
    popover_height: f64,
    facts: SharedFacts,
}

pub fn run() -> i32 {
    let relaunched = std::env::var_os(RELAUNCH_ENV).is_some();
    let lock = match SingleInstance::acquire_waiting(relaunched) {
        Acquire::Held(lock) => Some(lock),
        Acquire::Busy => {
            eprintln!("ai-usagebar-tray: another instance is already running.");
            return 0;
        }
        // A cache dir that cannot be written must not keep the tray from
        // starting; the cost is that two copies can then coexist.
        Acquire::Unavailable(reason) => {
            eprintln!(
                "ai-usagebar-tray: single-instance lock unavailable ({}); continuing.",
                crate::display::sanitize_untrusted_line(&reason)
            );
            None
        }
    };
    match run_loop(lock) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!(
                "ai-usagebar-tray: {}",
                crate::display::sanitize_untrusted_line(&error)
            );
            1
        }
    }
}

fn run_loop(lock: Option<SingleInstance>) -> Result<(), String> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // These run on the ksni thread: they only forward.
    {
        let proxy = proxy.clone();
        TrayIconEvent::set_event_handler(Some(move |event| {
            let _ = proxy.send_event(UserEvent::Tray(event));
        }));
    }
    {
        let proxy = proxy.clone();
        MenuEvent::set_event_handler(Some(move |event| {
            let _ = proxy.send_event(UserEvent::Menu(event));
        }));
    }

    let window = WindowBuilder::new()
        .with_title(WINDOW_TITLE)
        .with_inner_size(LogicalSize::new(panel::WINDOW_WIDTH, panel::WINDOW_HEIGHT))
        .with_visible(false)
        .with_decorations(false)
        .with_always_on_top(true)
        .with_resizable(false)
        .with_focused(false)
        .with_skip_taskbar(true)
        .with_theme(Some(Theme::Light.window_theme()))
        .build(&event_loop)
        .map_err(|error| error.to_string())?;

    let config = Config::load().unwrap_or_default();
    let facts: SharedFacts = Arc::new(Mutex::new(host_facts(&config)));
    // No global shortcut on this host: a configured one is reported as unavailable
    // instead of being silently dropped, the way a failed registration is.
    if let Some(configured) = config.tray.shortcut.as_deref() {
        let outcome = bind_shortcut(configured);
        with_facts(&facts, |f| apply_shortcut_outcome(f, outcome));
    }

    let (cmd_tx, cmd_rx) = mpsc::channel();
    spawn_worker(proxy.clone(), cmd_rx, facts.clone());
    let _ = cmd_tx.send(WorkerCmd::Refresh);

    let empty = wrap_report("{}", &facts_snapshot(&facts), now_ms(), None);
    let context_menu = Menu::new();
    options_menu::fill_menu(
        &context_menu,
        &options_menu::options_entries(&OptionsLabels::default(), false, startup::is_enabled()),
    );
    let tray = build_tray(&empty, &context_menu)?;

    let mut web_context = popover_web_context();
    let webview = build_webview(&mut web_context, &window, proxy.clone());
    let webview = match webview {
        Ok(webview) => Some(webview),
        Err(error) => {
            eprintln!(
                "ai-usagebar-tray: the popover WebView could not start: {}",
                crate::display::sanitize_untrusted_line(&error)
            );
            let _ = tray.set_tooltip(Some(WEBVIEW_MISSING));
            None
        }
    };

    let mut state = TrayState {
        window,
        webview,
        _web_context: web_context,
        tray,
        menu_labels: OptionsLabels::default(),
        pending_menu_action: None,
        context_menu,
        worker: cmd_tx,
        proxy: proxy.clone(),
        payload: empty,
        stars: Stars::new(),
        strip_order: Vec::new(),
        usage_reading: UsageReading::Used,
        js_ready: false,
        popover_open: false,
        last_shown: None,
        last_blur_hide: None,
        last_anchor: None,
        theme: Theme::Light,
        style: PopoverStyle::Classic,
        popover_height: panel::WINDOW_HEIGHT,
        facts,
    };

    event_loop.run(move |event, _, control_flow| {
        // The single-instance lock lives as long as the loop, which is the process.
        let _lock = &lock;
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UserEvent::Tray(tray_event)) => handle_tray(&mut state, tray_event),
            Event::UserEvent(UserEvent::Menu(menu_event)) => {
                handle_menu(&mut state, &menu_event, control_flow);
            }
            Event::UserEvent(UserEvent::Ipc(body)) => handle_ipc(&mut state, &body, control_flow),
            Event::UserEvent(UserEvent::Report(payload)) => apply_payload(&mut state, payload),
            Event::UserEvent(UserEvent::Facts) => apply_facts(&mut state),
            Event::UserEvent(UserEvent::Restart(exe)) => {
                if let Some(exe) = exe {
                    relaunch(&exe);
                }
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::Focused(false),
                ..
            } => {
                let since_shown = state.last_shown.map(|at| at.elapsed());
                if state.popover_open && blur_closes(since_shown) {
                    hide_popover(&mut state);
                    state.last_blur_hide = Some(Instant::now());
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => hide_popover(&mut state),
            Event::LoopDestroyed => {
                let _ = state.worker.send(WorkerCmd::Shutdown);
                // A sign-in CLI would otherwise outlive the tray, polling.
                super::device_login::stop_all();
            }
            _ => {}
        }
    });
}

/// Shown as the icon's tooltip when WebKitGTK could not start: the popover
/// cannot open, the icon and its menu still work.
const WEBVIEW_MISSING: &str = "WebKitGTK could not start, so the popover cannot open.";

// ---------------------------------------------------------------------------
// Pure decisions: no GTK, no D-Bus, no clock. Unit-tested below.
// ---------------------------------------------------------------------------

/// Whether an IPC message from the frame at `uri` may reach the bridge. The
/// bridge writes `config.toml`, so only the popover's own origin may use it.
fn ipc_allowed(uri: &str) -> bool {
    ipc_origin::is_popover_origin(uri, Os::Linux)
}

/// What the WebView may do with a navigation or `window.open` to `url`.
fn navigation_decision(url: &str) -> Navigation {
    ipc_origin::navigation(url, Os::Linux)
}

/// Whether a `Focused(false)` closes the popover. One that arrives within
/// [`BLUR_DEBOUNCE`] of the popover opening is the focus bounce of the click on
/// the icon itself, not the user going elsewhere. `None`: it never opened.
fn blur_closes(since_shown: Option<Duration>) -> bool {
    since_shown.is_none_or(|elapsed| elapsed >= BLUR_DEBOUNCE)
}

/// Whether a click on the icon opens the popover. A click right after a blur
/// closed it is the press that caused the blur: it stays closed.
fn click_reopens(since_blur_hide: Option<Duration>) -> bool {
    since_blur_hide.is_none_or(|elapsed| elapsed >= REOPEN_DEBOUNCE)
}

/// The click position a host reported, when it is usable: finite, and not the
/// `(0, 0)` some hosts send when they have no coordinates to give.
fn anchor_from_click(x: f64, y: f64) -> Option<(i32, i32)> {
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let (x, y) = (x.round() as i32, y.round() as i32);
    if x == 0 && y == 0 {
        return None;
    }
    Some((x, y))
}

/// A monitor's full rectangle and its work area (less panels and docks), in
/// physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Screen {
    full: Area,
    work: Area,
}

fn contains(area: Area, x: i32, y: i32) -> bool {
    x >= area.x && x < area.right() && y >= area.y && y < area.bottom()
}

/// Where a `width`×`height` popover goes, in physical pixels.
///
/// With an `anchor` (the click) on one of the `screens`: centered on it
/// horizontally, below it when it is in the upper half of that monitor and
/// above it otherwise (a panel at the bottom opens upward). Without a usable
/// anchor, or when no monitor holds it: the top-right corner of the `primary`
/// screen. No screen at all: [`DEFAULT_POSITION`]. The result is always held
/// inside the chosen monitor's work area, `POPOVER_MARGIN` clear of each edge;
/// a popover larger than that area keeps its top-left corner on the inset corner.
fn place_popover(
    anchor: Option<(i32, i32)>,
    screens: &[Screen],
    primary: Option<Screen>,
    width: i32,
    height: i32,
) -> (i32, i32) {
    let under_anchor = anchor.and_then(|(x, y)| {
        screens
            .iter()
            .copied()
            .find(|screen| contains(screen.full, x, y))
    });
    let Some(screen) = under_anchor.or(primary) else {
        return DEFAULT_POSITION;
    };
    let (x, y) = match anchor {
        Some((ax, ay)) if under_anchor.is_some() => {
            let upper_half = ay - screen.full.y < screen.full.height / 2;
            let y = if upper_half {
                ay + POPOVER_MARGIN
            } else {
                ay - height - POPOVER_MARGIN
            };
            (ax - width / 2, y)
        }
        _ => (
            screen.work.right() - width - POPOVER_MARGIN,
            screen.work.y + POPOVER_MARGIN,
        ),
    };
    let insets = Insets {
        left: POPOVER_MARGIN,
        top: POPOVER_MARGIN,
        right: POPOVER_MARGIN,
        bottom: POPOVER_MARGIN,
    };
    placement::clamp_into(screen.work, x, y, width, height, insets)
}

/// How the host treats an IPC command in this version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IpcKind {
    /// Served.
    Handled,
    /// A command another host serves that this one does not (yet): ignored, with
    /// no reply, so nothing pretends to have worked.
    NotInThisVersion,
    /// Not a command of this protocol at all: ignored.
    Unknown,
}

/// Commands the host serves. `set-shortcut` is here: it is answered, with the
/// shortcut unavailable. `install-update` is here too: the shared worker
/// refuses it on Linux (`installable`), which the page shows as the release link.
const HANDLED_COMMANDS: [&str; 24] = [
    "ready",
    "detect",
    "providers",
    "refresh",
    "set-provider-enabled",
    "sign-in-provider",
    "set-api-key",
    "start-sign-in",
    "cancel-sign-in",
    "open-sign-in-page",
    "copy-sign-in-code",
    "close",
    "quit",
    "toggle-startup",
    "menu-labels",
    "resize",
    "set-shortcut",
    "set-updates",
    "set-refresh",
    "strip",
    "open-url",
    "check-update",
    "install-update",
    "snooze-update",
];

/// Commands that exist in the protocol and are not served on Linux in this
/// version: the TUI launcher (no terminal detection yet) and the macOS-only
/// account, notification and menu-bar commands.
const UNSUPPORTED_COMMANDS: [&str; 9] = [
    "open-tui",
    "add-account",
    "switch-account",
    "prepare-account",
    "set-notifications-enabled",
    "set-notifications-threshold",
    "set-menu-bar-look",
    "set-menu-bar-short-name",
    "select-provider",
];

fn ipc_kind(cmd: &str) -> IpcKind {
    // `copy-image` has its own arm in `handle_ipc` and is listed here as well.
    if cmd == "copy-image" || HANDLED_COMMANDS.contains(&cmd) {
        IpcKind::Handled
    } else if UNSUPPORTED_COMMANDS.contains(&cmd) {
        IpcKind::NotInThisVersion
    } else {
        IpcKind::Unknown
    }
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

fn spawn_worker(
    proxy: EventLoopProxy<UserEvent>,
    rx: mpsc::Receiver<WorkerCmd>,
    facts: SharedFacts,
) {
    std::thread::Builder::new()
        .name("ai-usagebar-tray-fetch".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let Ok(rt) = rt else {
                return;
            };
            // First launch (and every provider that arrived with an update):
            // turn on the vendors whose credentials already exist locally.
            run_detection(false);
            let mut updates = {
                let announce = proxy.clone();
                let restart = proxy.clone();
                Updates::new(
                    facts.clone(),
                    Box::new(move || {
                        let _ = announce.send_event(UserEvent::Facts);
                    }),
                    Box::new(move |exe| {
                        let _ = restart.send_event(UserEvent::Restart(exe));
                    }),
                )
            };
            loop {
                rt.block_on(push_report(&proxy, &facts));
                if updates.due() {
                    rt.block_on(updates.check(false));
                }
                // Read per cycle so a `set-refresh` applies on the next one.
                let deadline =
                    Instant::now() + Duration::from_secs(facts_snapshot(&facts).refresh_secs);
                loop {
                    let wait = deadline.saturating_duration_since(Instant::now());
                    match rx.recv_timeout(wait) {
                        Ok(WorkerCmd::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Ok(WorkerCmd::Providers) => {
                            if super::providers::refresh_due(&facts) {
                                let _ = proxy.send_event(UserEvent::Facts);
                            }
                        }
                        Ok(WorkerCmd::Detect) => {
                            run_detection(true);
                            break;
                        }
                        Ok(WorkerCmd::CheckUpdate { manual }) => {
                            rt.block_on(updates.check(manual));
                        }
                        Ok(WorkerCmd::InstallUpdate) => rt.block_on(updates.install_or_check()),
                        Ok(WorkerCmd::SnoozeUpdate) => updates.snooze(),
                        Ok(WorkerCmd::SetUpdates(mode)) => {
                            rt.block_on(updates.set_mode(mode));
                        }
                        Ok(WorkerCmd::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            return;
                        }
                    }
                }
            }
        })
        .ok();
}

/// Best-effort: detection never blocks or fails the report. `force` re-probes
/// every vendor; otherwise only vendors this install has not seen before.
fn run_detection(force: bool) {
    if let Ok(state_path) = crate::detect::default_state_path() {
        let _ = crate::detect::run_once(None, &state_path, force);
    }
}

fn host_facts(config: &Config) -> HostFacts {
    let mut facts = HostFacts::new(env!("CARGO_PKG_VERSION"), startup::is_enabled());
    facts.updates = config.tray.updates().as_str().into();
    facts.refresh_secs = config.tray.refresh_minutes() * 60;
    facts
}

async fn push_report(proxy: &EventLoopProxy<UserEvent>, facts: &SharedFacts) {
    super::providers::refresh_due(facts);
    let mut snapshot = facts_snapshot(facts);
    snapshot.startup_enabled = startup::is_enabled();
    let now = now_ms();
    let payload = match crate::report::collect_json().await {
        Ok(json) => wrap_report(&json, &snapshot, now, None),
        Err(error) => wrap_report("{}", &snapshot, now, Some(&error)),
    };
    let _ = proxy.send_event(UserEvent::Report(payload));
}

fn relaunch(exe: &Path) {
    // Spawned and not waited on: the new tray outlives this one by design
    // (it waits for our lock, see `RELAUNCH_ENV`). Never reached on Linux today.
    let _ = std::process::Command::new(exe)
        .env(RELAUNCH_ENV, "1")
        .spawn();
}

// ---------------------------------------------------------------------------
// State updates
// ---------------------------------------------------------------------------

fn apply_payload(state: &mut TrayState, payload: Value) {
    state.payload = payload;
    refresh_icon(state);
    refresh_tooltip(state);
    stamp_facts(state);
    if state.js_ready {
        push_to_webview(state);
    }
}

fn refresh_tooltip(state: &mut TrayState) {
    if state.webview.is_none() {
        let _ = state.tray.set_tooltip(Some(WEBVIEW_MISSING));
        return;
    }
    let content = content_from_payload(&state.payload, &state.stars, &state.strip_order);
    let text = tooltip(&content, state.usage_reading);
    let _ = state.tray.set_tooltip(Some(&text));
}

/// Copy the shared facts onto the payload in place, so a shortcut or update
/// change shows up without waiting for the next report.
fn stamp_facts(state: &mut TrayState) {
    let facts = facts_snapshot(&state.facts);
    let stamped = wrap_report("{}", &facts, 0, None);
    let Some(obj) = state.payload.as_object_mut() else {
        return;
    };
    for key in super::payload::STAMPED_FACTS {
        obj.insert((*key).into(), stamped[*key].clone());
    }
}

fn apply_facts(state: &mut TrayState) {
    stamp_facts(state);
    if state.js_ready {
        push_to_webview(state);
    }
}

enum ShortcutOutcome {
    Cleared,
    Refused { attempted: String, reason: String },
}

/// This host registers no global shortcut. An empty value clears the setting;
/// anything else is refused with the reason, so the Settings row shows it
/// unavailable rather than bound.
fn bind_shortcut(text: &str) -> ShortcutOutcome {
    if text.trim().is_empty() {
        return ShortcutOutcome::Cleared;
    }
    match hotkey::normalize(text) {
        Ok(normalized) => ShortcutOutcome::Refused {
            attempted: normalized.canonical,
            reason: "Global shortcuts are not available on Linux yet.".into(),
        },
        Err(reason) => ShortcutOutcome::Refused {
            attempted: String::new(),
            reason,
        },
    }
}

fn apply_shortcut_outcome(facts: &mut HostFacts, outcome: ShortcutOutcome) {
    match outcome {
        ShortcutOutcome::Cleared => {
            facts.shortcut.clear();
            facts.shortcut_error.clear();
        }
        ShortcutOutcome::Refused { attempted, reason } => {
            facts.shortcut = attempted;
            facts.shortcut_error = reason;
        }
    }
}

fn config_path() -> Option<PathBuf> {
    crate::config::resolved_path().or_else(crate::config::default_path)
}

fn set_shortcut(state: &mut TrayState, value: &str) {
    let outcome = bind_shortcut(value);
    // Only a cleared shortcut is persisted; a refused one never was bound.
    if matches!(outcome, ShortcutOutcome::Cleared)
        && let Some(path) = config_path()
    {
        let _ = crate::config::set_tray_value(&path, "shortcut", None);
    }
    with_facts(&state.facts, |f| apply_shortcut_outcome(f, outcome));
    apply_facts(state);
}

fn set_updates(state: &mut TrayState, mode_text: &str) {
    let Some(mode) = UpdateMode::parse(mode_text) else {
        return;
    };
    if let Some(path) = config_path() {
        let _ = crate::config::set_tray_value(&path, "updates", Some(mode.as_str().into()));
    }
    let _ = state.worker.send(WorkerCmd::SetUpdates(mode));
}

/// `set-refresh {minutes}`: persist the poll interval and hand it to the
/// worker, which picks it up when it computes its next deadline.
fn set_refresh(state: &mut TrayState, minutes: u64) {
    if !crate::config::TRAY_REFRESH_MINUTES.contains(&minutes) {
        return;
    }
    if let Some(path) = config_path() {
        let _ = crate::config::set_tray_value(
            &path,
            "refresh_minutes",
            Some(i64::try_from(minutes).unwrap_or(i64::MAX).into()),
        );
    }
    with_facts(&state.facts, |f| f.refresh_secs = minutes * 60);
    apply_facts(state);
    let _ = state.worker.send(WorkerCmd::Refresh);
}

fn push_to_webview(state: &TrayState) {
    let Some(webview) = state.webview.as_ref() else {
        return;
    };
    let json = host_payload(&state.payload);
    let script = format!("window.__AIUB_APPLY__ && window.__AIUB_APPLY__({json})");
    let _ = webview.evaluate_script(&script);
}

/// The `copy-image` answer, built by `clipboard_image::copied_reply_script`
/// with the request's id so the page matches it to its own request.
fn reply_image_copied(state: &TrayState, id: Option<&str>, copied: bool) {
    if let Some(webview) = state.webview.as_ref() {
        let _ = webview.evaluate_script(&super::clipboard_image::copied_reply_script(id, copied));
    }
}

// ---------------------------------------------------------------------------
// Clipboard (GTK, main thread only)
// ---------------------------------------------------------------------------

/// Plain text on the CLIPBOARD selection: the sign-in sheet's one-time code.
/// GTK reports no failure for a set, so `true` means it was handed over.
fn copy_text_to_clipboard(text: &str) -> bool {
    let clipboard = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
    clipboard.set_text(text);
    // Ask the clipboard manager to keep it past this process, if one runs.
    clipboard.store();
    true
}

/// A PNG, already through `clipboard_image::png_from_ipc`'s size and format
/// checks, decoded by gdk-pixbuf and set as an image. `false` when the decoder
/// refuses the bytes.
fn copy_png_to_clipboard(bytes: &[u8]) -> bool {
    let Some(pixbuf) = decode_png(bytes) else {
        return false;
    };
    let clipboard = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
    clipboard.set_image(&pixbuf);
    clipboard.store();
    true
}

fn decode_png(bytes: &[u8]) -> Option<gdk_pixbuf::Pixbuf> {
    let loader = gdk_pixbuf::PixbufLoader::with_type("png").ok()?;
    let written = loader.write(bytes);
    // Closed on every path: a loader dropped open complains and leaks its state.
    let closed = loader.close();
    if written.is_err() || closed.is_err() {
        return None;
    }
    loader.pixbuf()
}

// ---------------------------------------------------------------------------
// Tray, menu, IPC
// ---------------------------------------------------------------------------

fn handle_tray(state: &mut TrayState, event: TrayIconEvent) {
    // The right-click menu belongs to the SNI host: it never reaches here.
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        position,
        ..
    } = event
    {
        let anchor = anchor_from_click(position.x, position.y);
        let since_blur_hide = state.last_blur_hide.take().map(|at| at.elapsed());
        if state.popover_open {
            hide_popover(state);
        } else if click_reopens(since_blur_hide) {
            show_popover(state, anchor);
        }
    }
}

fn handle_menu(state: &mut TrayState, event: &MenuEvent, control_flow: &mut ControlFlow) {
    let Some(action) = OptionsAction::from_id(event.id.as_ref()) else {
        return;
    };
    match action {
        OptionsAction::Refresh => {
            let _ = state.worker.send(WorkerCmd::Refresh);
        }
        OptionsAction::Detect => {
            super::providers::request();
            let _ = state.worker.send(WorkerCmd::Detect);
        }
        // Not in this version: no terminal launcher yet (see `UNSUPPORTED_COMMANDS`).
        OptionsAction::OpenTui => {}
        OptionsAction::ToggleStartup => toggle_startup(state),
        OptionsAction::Quit => *control_flow = ControlFlow::Exit,
        OptionsAction::Customize
        | OptionsAction::Settings
        | OptionsAction::CheckUpdates
        | OptionsAction::About => open_popover_action(state, action),
    }
}

fn handle_ipc(state: &mut TrayState, body: &str, control_flow: &mut ControlFlow) {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return;
    };
    let cmd = value.get("cmd").and_then(Value::as_str).unwrap_or("");
    if ipc_kind(cmd) != IpcKind::Handled {
        return;
    }
    match cmd {
        "ready" => {
            state.js_ready = true;
            push_to_webview(state);
            if let Some(screen) = state.pending_menu_action.take() {
                run_menu_action(state, screen);
            }
        }
        "detect" => {
            super::providers::request();
            let _ = state.worker.send(WorkerCmd::Detect);
        }
        "providers" => {
            super::providers::request();
            let _ = state.worker.send(WorkerCmd::Providers);
        }
        "refresh" => {
            let _ = state.worker.send(WorkerCmd::Refresh);
        }
        "set-provider-enabled" => {
            super::providers::set_enabled(&value, &state.facts);
            apply_facts(state);
            let _ = state.worker.send(WorkerCmd::Refresh);
        }
        "sign-in-provider" => super::providers::sign_in(&value),
        "set-api-key" => {
            let proxy = state.proxy.clone();
            let worker = state.worker.clone();
            match super::providers::request_key_check(&state.facts, &value, move || {
                let _ = proxy.send_event(UserEvent::Facts);
                let _ = worker.send(WorkerCmd::Refresh);
            }) {
                // `Ok(None)` is an ignored request: nothing to publish.
                Ok(_) => apply_facts(state),
                Err(fact) => {
                    with_facts(&state.facts, |f| f.key_check = Some(fact));
                    apply_facts(state);
                }
            }
        }
        "start-sign-in" => {
            let push = state.proxy.clone();
            let worker = state.worker.clone();
            super::device_login::start(
                &state.facts,
                &value,
                move || {
                    let _ = push.send_event(UserEvent::Facts);
                },
                move || {
                    let _ = worker.send(WorkerCmd::Refresh);
                },
            );
            apply_facts(state);
        }
        "cancel-sign-in" => {
            if super::device_login::cancel(&state.facts, &value) {
                apply_facts(state);
            }
        }
        "open-sign-in-page" => super::device_login::open_page(&state.facts, &value),
        "copy-sign-in-code" => {
            if let Some(code) = super::device_login::code_to_copy(&state.facts, &value) {
                copy_text_to_clipboard(&code);
            }
        }
        "close" => hide_popover(state),
        "quit" => *control_flow = ControlFlow::Exit,
        "toggle-startup" => toggle_startup(state),
        "menu-labels" => {
            state.menu_labels = state.menu_labels.merged(&value);
            refresh_menu(state);
        }
        "resize" => handle_resize(state, &value),
        "set-shortcut" => {
            let text = value.get("value").and_then(Value::as_str).unwrap_or("");
            set_shortcut(state, text);
        }
        "set-updates" => {
            let mode = value.get("mode").and_then(Value::as_str).unwrap_or("");
            set_updates(state, mode);
        }
        "set-refresh" => {
            if let Some(minutes) = value.get("minutes").and_then(Value::as_u64) {
                set_refresh(state, minutes);
            }
        }
        "strip" => {
            let (_, stars, order) = parse_strip_ipc(&value);
            state.stars = stars;
            state.strip_order = order;
            state.usage_reading = UsageReading::from_strip_ipc(&value);
            refresh_tooltip(state);
        }
        "open-url" => {
            if let Some(url) = value.get("url").and_then(Value::as_str) {
                browse::open(url);
            }
        }
        "check-update" => {
            let _ = state.worker.send(WorkerCmd::CheckUpdate { manual: true });
        }
        "install-update" => {
            let _ = state.worker.send(WorkerCmd::InstallUpdate);
        }
        "snooze-update" => {
            let _ = state.worker.send(WorkerCmd::SnoozeUpdate);
        }
        "copy-image" => {
            let id = super::clipboard_image::copy_request_id(&value);
            // No valid id: no `png_from_ipc`, no clipboard — the reply is `null, false`.
            let copied = match id {
                Some(_) => match super::clipboard_image::png_from_ipc(&value) {
                    Ok(bytes) => copy_png_to_clipboard(&bytes),
                    Err(_reason) => false,
                },
                None => false,
            };
            reply_image_copied(state, id, copied);
        }
        _ => {}
    }
}

/// `{"cmd":"resize","height":<logical px>,"theme":"light"|"dark","style":"classic"|"native"}`.
/// `theme` and `style` are optional; a missing or malformed `height` is ignored.
fn handle_resize(state: &mut TrayState, value: &Value) {
    if let Some(theme) = value
        .get("theme")
        .and_then(Value::as_str)
        .and_then(Theme::parse)
    {
        apply_theme(state, theme);
    }
    let mut resize = false;
    if let Some(style) = value
        .get("style")
        .and_then(Value::as_str)
        .and_then(PopoverStyle::parse)
        && state.style != style
    {
        // No material on Linux: `Native` is tracked (it sets the page's layout
        // width and drops Customize from the menu) but never painted differently.
        state.style = style;
        refresh_menu(state);
        resize = true;
    }
    if let Some(requested) = value.get("height").and_then(Value::as_f64)
        && requested.is_finite()
        && requested > 0.0
    {
        state.popover_height =
            panel::clamp_popover_height(requested, work_area_height(&state.window));
        resize = true;
    }
    if resize {
        state.window.set_inner_size(LogicalSize::new(
            state.style.window_width(panel::WINDOW_WIDTH),
            state.popover_height,
        ));
        if state.popover_open {
            position_window(&state.window, state.last_anchor);
        }
    }
}

fn apply_theme(state: &mut TrayState, theme: Theme) {
    if state.theme == theme {
        return;
    }
    state.theme = theme;
    state.window.set_theme(Some(theme.window_theme()));
    if let Some(webview) = state.webview.as_ref() {
        let _ = webview.set_background_color(theme.background());
    }
}

fn toggle_startup(state: &mut TrayState) {
    let next = !startup::is_enabled();
    match startup::set_enabled(next) {
        Ok(()) => {
            if let Some(obj) = state.payload.as_object_mut() {
                obj.insert("startup_enabled".into(), Value::Bool(next));
            }
            if state.js_ready {
                push_to_webview(state);
            }
            refresh_menu(state);
        }
        Err(error) => eprintln!(
            "ai-usagebar-tray: start at login was not changed: {}",
            crate::display::sanitize_untrusted_line(&error)
        ),
    }
}

/// Refill the one menu object; the ksni snapshot picks the change up and the
/// panel's menu follows. The style flag drops Customize in `Native`.
fn refresh_menu(state: &TrayState) {
    options_menu::fill_menu(
        &state.context_menu,
        &options_menu::options_entries(
            &state.menu_labels,
            state.style == PopoverStyle::Native,
            startup::is_enabled(),
        ),
    );
}

/// The navigation itself: the popover installs `__AIUB_MENU_ACTION__` on load,
/// so this only reaches the page once there is a webview to evaluate in.
fn run_menu_action(state: &TrayState, screen: &str) {
    if let Some(webview) = state.webview.as_ref() {
        let _ = webview.evaluate_script(&format!(
            "window.__AIUB_MENU_ACTION__ && window.__AIUB_MENU_ACTION__({})",
            serde_json::json!(screen)
        ));
    }
}

/// A menu entry that opens the popover on a given screen: show it when closed,
/// then hand the page the navigation. Before the page reports `ready` the hook
/// does not exist, so the choice is kept and runs on `ready`.
fn open_popover_action(state: &mut TrayState, action: OptionsAction) {
    if state.webview.is_none() {
        return;
    }
    let Some(screen) = action.popover_action() else {
        return;
    };
    if !state.popover_open {
        show_popover(state, state.last_anchor);
    }
    if state.js_ready {
        run_menu_action(state, screen);
    } else {
        state.pending_menu_action = Some(screen);
    }
}

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

fn show_popover(state: &mut TrayState, anchor: Option<(i32, i32)>) {
    if anchor.is_some() {
        state.last_anchor = anchor;
    }
    position_window(&state.window, state.last_anchor);
    state.window.set_visible(true);
    state.popover_open = true;
    state.last_shown = Some(Instant::now());
    if let Some(webview) = state.webview.as_ref() {
        let _ = webview.evaluate_script(&format!(
            "window.__AIUB_LOCKCLICKS__ && window.__AIUB_LOCKCLICKS__({CLICK_LOCK_MS})"
        ));
        let _ = webview.evaluate_script("window.__AIUB_VISIBLE__ && window.__AIUB_VISIBLE__(true)");
    }
    if state.js_ready {
        push_to_webview(state);
    }
    state.window.set_focus();
}

fn hide_popover(state: &mut TrayState) {
    state.window.set_visible(false);
    state.popover_open = false;
    // Closing resets navigation so the next open lands on the dashboard — a
    // screen choice still waiting for `ready` included.
    state.pending_menu_action = None;
    if let Some(webview) = state.webview.as_ref() {
        let _ =
            webview.evaluate_script("window.__AIUB_VISIBLE__ && window.__AIUB_VISIBLE__(false)");
    }
}

/// The monitor's full rectangle and work area in physical pixels. tao gives the
/// full one; GDK's `workarea` (logical px) is scaled up to match, and the full
/// rectangle stands in for it if GDK reports nothing usable.
fn screen_of(monitor: &MonitorHandle) -> Screen {
    let origin = monitor.position();
    let size = monitor.size();
    let full = Area {
        x: origin.x,
        y: origin.y,
        width: i32::try_from(size.width).unwrap_or(i32::MAX),
        height: i32::try_from(size.height).unwrap_or(i32::MAX),
    };
    let scale = monitor.scale_factor();
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let area = monitor.gdk_monitor().workarea();
    let work = Area {
        x: (f64::from(area.x()) * scale).round() as i32,
        y: (f64::from(area.y()) * scale).round() as i32,
        width: (f64::from(area.width()) * scale).round() as i32,
        height: (f64::from(area.height()) * scale).round() as i32,
    };
    if work.width <= 0 || work.height <= 0 {
        return Screen { full, work: full };
    }
    Screen { full, work }
}

/// Logical inner height the popover may take on the monitor it sits on (primary
/// as fallback): that monitor's work area, so it never runs under a panel.
fn work_area_height(window: &Window) -> f64 {
    let Some(monitor) = window
        .current_monitor()
        .or_else(|| window.primary_monitor())
    else {
        return panel::FALLBACK_WORK_AREA_HEIGHT;
    };
    let screen = screen_of(&monitor);
    let frame = i32::try_from(window.outer_size().height).unwrap_or(0)
        - i32::try_from(window.inner_size().height).unwrap_or(0);
    placement::max_inner_height(screen.work, frame, monitor.scale_factor())
}

fn position_window(window: &Window, anchor: Option<(i32, i32)>) {
    let size = window.outer_size();
    let (width, height) = (
        i32::try_from(size.width).unwrap_or(i32::MAX),
        i32::try_from(size.height).unwrap_or(i32::MAX),
    );
    let screens: Vec<Screen> = window
        .available_monitors()
        .map(|monitor| screen_of(&monitor))
        .collect();
    let primary = window.primary_monitor().map(|monitor| screen_of(&monitor));
    let (x, y) = place_popover(anchor, &screens, primary, width, height);
    // A Wayland compositor may ignore this; the popover is then wherever it puts it.
    window.set_outer_position(PhysicalPosition::new(x, y));
}

// ---------------------------------------------------------------------------
// Icon and WebView
// ---------------------------------------------------------------------------

fn icon_from_severity(severity: Severity) -> Result<Icon, tray_icon::BadIcon> {
    let (mut rgba, size) = tray_icon_rgba(ICON_SIDE, severity);
    apply_ink(&mut rgba, ICON_INK);
    Icon::from_rgba(rgba, size, size)
}

fn build_tray(payload: &Value, menu: &Menu) -> Result<TrayIcon, String> {
    let icon = icon_from_severity(worst_severity(payload)).map_err(|error| error.to_string())?;
    TrayIconBuilder::new()
        .with_icon(icon)
        .with_title(WINDOW_TITLE)
        .with_tooltip(WINDOW_TITLE)
        .with_menu(Box::new(menu.clone()))
        .with_menu_on_left_click(false)
        .build()
        .map_err(|error| error.to_string())
}

/// Redraw the tray glyph for the current report.
fn refresh_icon(state: &mut TrayState) {
    if let Ok(icon) = icon_from_severity(worst_severity(&state.payload)) {
        let _ = state.tray.set_icon(Some(icon));
    }
}

/// The WebKit data directory, pinned under the cache root (see `profile`) so
/// the popover's layout (`localStorage`) survives restarts. If the folder
/// cannot be created the context carries no directory and WebKit uses its
/// default: the popover still opens, only without a stable profile.
fn popover_web_context() -> WebContext {
    let dir = crate::cache::xdg_cache_dir()
        .ok()
        .map(|root| profile::popover_data_dir(&root))
        .filter(|dir| std::fs::create_dir_all(dir).is_ok());
    WebContext::new(dir)
}

fn build_webview(
    web_context: &mut WebContext,
    window: &Window,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<WebView, String> {
    let vbox = window
        .default_vbox()
        .ok_or_else(|| "the window has no GTK container for the WebView".to_string())?;
    WebViewBuilder::new_with_web_context(web_context)
        .with_custom_protocol("aiub".into(), move |_id, request| {
            protocol_response(request)
        })
        .with_url(POPOVER_URL)
        .with_background_color(Theme::Light.background())
        .with_ipc_handler(move |request| {
            // wry stamps the message with the sending script's absolute URL; any
            // other origin is dropped before the parse, without a reply.
            if !ipc_allowed(&request.uri().to_string()) {
                return;
            }
            // Dropped before the clone and the queue: an oversized body never
            // reaches the UI thread's parse.
            if !ipc_body_within_limit(request.body().len()) {
                return;
            }
            let body = request.body().clone();
            let _ = proxy.send_event(UserEvent::Ipc(body));
        })
        // The WebView never leaves the popover's origin: other web URLs go to
        // the default browser (the `open-url` command's path), the rest is denied.
        .with_navigation_handler(|url| navigation_allowed(&url))
        .with_new_window_req_handler(|url, _features| {
            if navigation_allowed(&url) {
                NewWindowResponse::Allow
            } else {
                NewWindowResponse::Deny
            }
        })
        .build_gtk(vbox)
        .map_err(|error| error.to_string())
}

/// Applies [`navigation_decision`]: the popover's own origin loads in place;
/// another web URL goes to `browse::open` (a detached `xdg-open`, spawned and
/// not waited on, so no child is held) and is denied in place.
fn navigation_allowed(url: &str) -> bool {
    match navigation_decision(url) {
        Navigation::Allow => true,
        Navigation::OpenExternal => {
            browse::open(url);
            false
        }
        Navigation::Deny => false,
    }
}

/// Largest IPC message the host accepts: the 16 MiB PNG limit of
/// `copy-image`, as base64 (about 21.4 MiB), plus the JSON envelope.
const MAX_IPC_BODY_BYTES: usize = 24 * 1024 * 1024;

fn ipc_body_within_limit(len: usize) -> bool {
    len <= MAX_IPC_BODY_BYTES
}

/// Sent on every 200 from the `aiub` protocol: the popover has no use for
/// frames, plugins or embedded documents, and a frame would reach the IPC
/// bridge under the main frame's URL (wry reports that URL for subframes).
/// Script, style, image and connect sources are deliberately left open.
const POPOVER_CSP: &str =
    "frame-src 'none'; child-src 'none'; object-src 'none'; frame-ancestors 'none'";

fn protocol_response(request: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    asset_response(request.uri().path())
}

fn asset_response(path: &str) -> Response<Cow<'static, [u8]>> {
    let (body, mime): (&'static [u8], &str) = match path {
        "/" | "/index.html" => (INDEX_HTML.as_bytes(), "text/html; charset=utf-8"),
        "/popover.css" => (POPOVER_CSS.as_bytes(), "text/css; charset=utf-8"),
        "/popover.js" => (POPOVER_JS.as_bytes(), "text/javascript; charset=utf-8"),
        _ => {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Cow::Borrowed(b"" as &[u8]))
                .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"" as &[u8])));
        }
    };
    Response::builder()
        .header(CONTENT_TYPE, mime)
        .header("Access-Control-Allow-Origin", "*")
        .header("Content-Security-Policy", POPOVER_CSP)
        .body(Cow::Borrowed(body))
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(body)))
}

// ---------------------------------------------------------------------------
// Single instance
// ---------------------------------------------------------------------------

/// Outcome of trying to take the per-user lock.
enum Acquire {
    Held(SingleInstance),
    /// Another process of this user holds it.
    Busy,
    /// The lock file could not be opened or locked for a reason other than
    /// contention (unwritable cache dir, no home): the caller decides.
    Unavailable(String),
}

/// An exclusive, non-blocking `flock` on `tray.lock` in the user's cache
/// directory. The kernel drops it when the process ends, however it ends, so a
/// crash leaves nothing stale; it is per user because the directory is.
struct SingleInstance {
    _file: File,
}

impl SingleInstance {
    /// A relaunch after an update races the old process's exit; keep retrying
    /// for a bounded time instead of silently quitting.
    fn acquire_waiting(wait: bool) -> Acquire {
        let path = match lock_path() {
            Ok(path) => path,
            Err(reason) => return Acquire::Unavailable(reason),
        };
        let first = Self::acquire_at(&path);
        if !wait || !matches!(first, Acquire::Busy) {
            return first;
        }
        let deadline = Instant::now() + RELAUNCH_WAIT;
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let attempt = Self::acquire_at(&path);
            if !matches!(attempt, Acquire::Busy) || Instant::now() >= deadline {
                return attempt;
            }
        }
    }

    fn acquire_at(path: &Path) -> Acquire {
        // Before opening: a link, FIFO, directory or device at the lock path is
        // never opened (opening a FIFO for writing can block before the flock).
        match std::fs::symlink_metadata(path) {
            Ok(meta) if !meta.file_type().is_file() => {
                return Acquire::Unavailable("the lock path is not a regular file".to_string());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Acquire::Unavailable(error.to_string()),
        }
        if let Some(parent) = path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            return Acquire::Unavailable(error.to_string());
        }
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).write(true);
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) => return Acquire::Unavailable(error.to_string()),
        };
        // After opening: the descriptor itself must be our own regular file.
        let verdict = file
            .metadata()
            .map_err(|error| error.to_string())
            .and_then(|meta| lock_file_verdict(&meta, rustix::process::getuid().as_raw()));
        if let Err(reason) = verdict {
            return Acquire::Unavailable(reason);
        }
        match file.try_lock_exclusive() {
            Ok(()) => Acquire::Held(Self { _file: file }),
            Err(error) if is_contended(&error) => Acquire::Busy,
            Err(error) => Acquire::Unavailable(error.to_string()),
        }
    }
}

/// A lock file is trusted only when it is a regular file owned by `uid`.
fn lock_file_verdict(meta: &std::fs::Metadata, uid: u32) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt as _;
    if !meta.file_type().is_file() {
        return Err("the lock path is not a regular file".to_string());
    }
    if meta.uid() != uid {
        return Err("the lock file belongs to another user".to_string());
    }
    Ok(())
}

fn is_contended(error: &std::io::Error) -> bool {
    error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

fn lock_path() -> Result<PathBuf, String> {
    let root = crate::cache::xdg_cache_dir().map_err(|error| error.to_string())?;
    Ok(root.join("ai-usagebar").join("tray.lock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: Screen = Screen {
        full: Area {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        },
        work: Area {
            x: 0,
            y: 30,
            width: 1920,
            height: 1050,
        },
    };

    /// A second monitor to the right of [`MONITOR`].
    const RIGHT: Screen = Screen {
        full: Area {
            x: 1920,
            y: 0,
            width: 2560,
            height: 1440,
        },
        work: Area {
            x: 1920,
            y: 0,
            width: 2560,
            height: 1440,
        },
    };

    fn inside(work: Area, (x, y): (i32, i32), width: i32, height: i32) -> bool {
        x >= work.x + POPOVER_MARGIN
            && y >= work.y + POPOVER_MARGIN
            && x + width <= work.right() - POPOVER_MARGIN
            && y + height <= work.bottom() - POPOVER_MARGIN
    }

    // --- IPC origin filter -------------------------------------------------

    #[test]
    fn ipc_accepts_only_the_popover_origin() {
        assert!(ipc_allowed("aiub://localhost/index.html"));
        assert!(ipc_allowed("aiub://localhost/"));
        assert!(ipc_allowed("AIUB://LOCALHOST/popover.js"));
    }

    #[test]
    fn ipc_drops_the_windows_origin_and_every_hostile_variant() {
        for uri in [
            // The Windows host's origin is not this host's.
            "http://aiub.localhost/index.html",
            "https://aiub.localhost/index.html",
            "aiub://localhost.evil/index.html",
            "aiub://evil.localhost/index.html",
            "aiub://localhost:8080/index.html",
            "aiub://user@evil/index.html",
            "aiub:localhost/index.html",
            "aiub:///index.html",
            "file:///tmp/popover/index.html",
            "https://example.com/",
            "javascript:window.ipc.postMessage('x')",
            "about:blank",
            "data:text/html,hi",
            "",
            "localhost",
        ] {
            assert!(!ipc_allowed(uri), "{uri:?} must not reach the bridge");
        }
    }

    // --- Navigation --------------------------------------------------------

    #[test]
    fn navigation_keeps_the_popover_origin_and_sends_the_web_out() {
        assert_eq!(
            navigation_decision("aiub://localhost/index.html"),
            Navigation::Allow
        );
        assert_eq!(
            navigation_decision("https://status.anthropic.com/"),
            Navigation::OpenExternal
        );
        // The Windows origin is an ordinary web URL here, not the popover.
        assert_eq!(
            navigation_decision("http://aiub.localhost/index.html"),
            Navigation::OpenExternal
        );
    }

    #[test]
    fn navigation_denies_everything_that_is_not_a_web_url() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,x",
            "ftp://example.com/",
            "aiub://localhost.evil/",
            "https://ok.example\nhttps://evil.example",
            "",
        ] {
            assert_eq!(navigation_decision(url), Navigation::Deny, "{url:?}");
        }
    }

    // --- Focus debounce ----------------------------------------------------

    #[test]
    fn a_blur_right_after_opening_is_ignored() {
        assert!(!blur_closes(Some(Duration::ZERO)));
        assert!(!blur_closes(Some(BLUR_DEBOUNCE - Duration::from_millis(1))));
    }

    #[test]
    fn a_blur_after_the_debounce_closes() {
        assert!(blur_closes(Some(BLUR_DEBOUNCE)));
        assert!(blur_closes(Some(Duration::from_secs(5))));
    }

    #[test]
    fn a_blur_with_no_recorded_opening_closes() {
        assert!(blur_closes(None));
    }

    #[test]
    fn a_click_right_after_a_blur_close_stays_closed() {
        assert!(!click_reopens(Some(Duration::ZERO)));
        assert!(!click_reopens(Some(
            REOPEN_DEBOUNCE - Duration::from_millis(1)
        )));
    }

    #[test]
    fn a_later_click_reopens() {
        assert!(click_reopens(Some(REOPEN_DEBOUNCE)));
        assert!(click_reopens(Some(Duration::from_secs(30))));
        assert!(click_reopens(None));
    }

    // --- Click anchor ------------------------------------------------------

    #[test]
    fn a_click_position_is_used_when_it_is_real() {
        assert_eq!(anchor_from_click(1800.4, 12.6), Some((1800, 13)));
        assert_eq!(anchor_from_click(-40.0, 5.0), Some((-40, 5)));
        assert_eq!(anchor_from_click(0.0, 17.0), Some((0, 17)));
    }

    #[test]
    fn a_click_with_no_coordinates_is_no_anchor() {
        assert_eq!(anchor_from_click(0.0, 0.0), None);
        assert_eq!(anchor_from_click(f64::NAN, 10.0), None);
        assert_eq!(anchor_from_click(10.0, f64::INFINITY), None);
    }

    // --- Position ----------------------------------------------------------

    #[test]
    fn a_click_in_the_upper_half_opens_below_it_centered() {
        let (x, y) = place_popover(Some((960, 15)), &[MONITOR], Some(MONITOR), 320, 420);
        assert_eq!(x, 960 - 160);
        // Below the click would be y = 23, which is over the panel (work area
        // starts at 30): it is held at the work area's inset instead.
        assert_eq!(y, MONITOR.work.y + POPOVER_MARGIN);
    }

    #[test]
    fn a_click_in_the_lower_half_opens_above_it() {
        let (x, y) = place_popover(Some((960, 1060)), &[MONITOR], Some(MONITOR), 320, 420);
        assert_eq!(x, 800);
        assert_eq!(y, 1060 - 420 - POPOVER_MARGIN);
    }

    #[test]
    fn a_click_near_the_right_edge_is_pulled_back_inside() {
        let width = 320;
        let (x, y) = place_popover(Some((1915, 14)), &[MONITOR], Some(MONITOR), width, 420);
        assert_eq!(x, MONITOR.work.right() - width - POPOVER_MARGIN);
        assert!(inside(MONITOR.work, (x, y), width, 420));
    }

    #[test]
    fn a_click_near_the_bottom_edge_is_pulled_back_inside() {
        let (x, y) = place_popover(Some((10, 1079)), &[MONITOR], Some(MONITOR), 320, 420);
        assert!(inside(MONITOR.work, (x, y), 320, 420));
        assert_eq!(x, MONITOR.work.x + POPOVER_MARGIN);
    }

    #[test]
    fn the_clamp_holds_the_popover_to_the_monitor_under_the_click() {
        // A click on the right monitor's far edge must not spill onto nothing,
        // nor back onto the left monitor.
        let (x, y) = place_popover(Some((4479, 20)), &[MONITOR, RIGHT], Some(MONITOR), 320, 420);
        assert!(inside(RIGHT.work, (x, y), 320, 420));
        assert_eq!(x, RIGHT.work.right() - 320 - POPOVER_MARGIN);
    }

    #[test]
    fn a_popover_taller_than_the_monitor_keeps_its_top_left_on_the_inset_corner() {
        let (x, y) = place_popover(Some((960, 600)), &[MONITOR], Some(MONITOR), 320, 4000);
        assert_eq!(y, MONITOR.work.y + POPOVER_MARGIN);
        assert!(x >= MONITOR.work.x + POPOVER_MARGIN);
        let (x, _) = place_popover(Some((960, 600)), &[MONITOR], Some(MONITOR), 5000, 420);
        assert_eq!(x, MONITOR.work.x + POPOVER_MARGIN);
    }

    #[test]
    fn a_click_on_no_monitor_falls_back_to_the_primary_corner() {
        let (x, y) = place_popover(Some((-500, -500)), &[MONITOR], Some(MONITOR), 320, 420);
        assert_eq!(x, MONITOR.work.right() - 320 - POPOVER_MARGIN);
        assert_eq!(y, MONITOR.work.y + POPOVER_MARGIN);
    }

    #[test]
    fn no_anchor_opens_in_the_primary_top_right_corner() {
        let position = place_popover(None, &[MONITOR, RIGHT], Some(MONITOR), 320, 420);
        assert_eq!(
            position,
            (
                MONITOR.work.right() - 320 - POPOVER_MARGIN,
                MONITOR.work.y + POPOVER_MARGIN
            )
        );
    }

    #[test]
    fn no_monitor_at_all_gives_the_default_position() {
        assert_eq!(place_popover(None, &[], None, 320, 420), DEFAULT_POSITION);
        assert_eq!(
            place_popover(Some((100, 100)), &[], None, 320, 420),
            DEFAULT_POSITION
        );
    }

    #[test]
    fn a_click_on_a_monitor_the_list_lacks_uses_the_primary_not_the_default() {
        let (x, y) = place_popover(Some((100, 100)), &[], Some(MONITOR), 320, 420);
        assert!(inside(MONITOR.work, (x, y), 320, 420));
    }

    // --- IPC command classes ----------------------------------------------

    #[test]
    fn the_core_commands_are_handled() {
        for cmd in HANDLED_COMMANDS {
            assert_eq!(ipc_kind(cmd), IpcKind::Handled, "{cmd}");
        }
        assert_eq!(ipc_kind("copy-image"), IpcKind::Handled);
    }

    #[test]
    fn commands_outside_the_first_version_are_flagged_not_handled() {
        for cmd in UNSUPPORTED_COMMANDS {
            assert_eq!(ipc_kind(cmd), IpcKind::NotInThisVersion, "{cmd}");
        }
    }

    #[test]
    fn unknown_and_hostile_commands_are_unknown() {
        for cmd in [
            "",
            "READY",
            "ready ",
            "rm -rf",
            "../quit",
            "open-tui\0",
            "__proto__",
        ] {
            assert_eq!(ipc_kind(cmd), IpcKind::Unknown, "{cmd:?}");
        }
    }

    #[test]
    fn handled_and_unsupported_lists_do_not_overlap() {
        for cmd in UNSUPPORTED_COMMANDS {
            assert!(!HANDLED_COMMANDS.contains(&cmd), "{cmd}");
        }
    }

    // --- Shortcut ----------------------------------------------------------

    #[test]
    fn a_shortcut_is_refused_with_a_reason_and_never_bound() {
        let ShortcutOutcome::Refused { attempted, reason } = bind_shortcut("ctrl+shift+u") else {
            panic!("a shortcut must be refused on this host");
        };
        assert!(!attempted.is_empty());
        assert!(!reason.is_empty());
        let mut facts = HostFacts::new("0.0.0", false);
        apply_shortcut_outcome(&mut facts, ShortcutOutcome::Refused { attempted, reason });
        assert!(!facts.shortcut_error.is_empty());
    }

    #[test]
    fn an_empty_shortcut_clears_the_setting() {
        assert!(matches!(bind_shortcut("  "), ShortcutOutcome::Cleared));
        let mut facts = HostFacts::new("0.0.0", false);
        facts.shortcut = "Ctrl+U".into();
        facts.shortcut_error = "old".into();
        apply_shortcut_outcome(&mut facts, ShortcutOutcome::Cleared);
        assert!(facts.shortcut.is_empty() && facts.shortcut_error.is_empty());
    }

    #[test]
    fn an_unparsable_shortcut_keeps_the_parser_reason() {
        let ShortcutOutcome::Refused { attempted, reason } = bind_shortcut("not a shortcut ++")
        else {
            panic!("must be refused");
        };
        assert!(attempted.is_empty());
        assert!(!reason.is_empty());
    }

    // --- Single instance ---------------------------------------------------

    #[test]
    fn the_second_holder_of_the_lock_is_refused_until_the_first_drops_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("nested").join("tray.lock");
        let first = SingleInstance::acquire_at(&path);
        assert!(matches!(first, Acquire::Held(_)));
        assert!(matches!(SingleInstance::acquire_at(&path), Acquire::Busy));
        drop(first);
        assert!(matches!(
            SingleInstance::acquire_at(&path),
            Acquire::Held(_)
        ));
    }

    #[test]
    fn two_users_lock_files_do_not_block_each_other() {
        let a = tempfile::tempdir().expect("temp dir");
        let b = tempfile::tempdir().expect("temp dir");
        let first = SingleInstance::acquire_at(&a.path().join("tray.lock"));
        let second = SingleInstance::acquire_at(&b.path().join("tray.lock"));
        assert!(matches!(first, Acquire::Held(_)));
        assert!(matches!(second, Acquire::Held(_)));
    }

    #[test]
    fn an_unwritable_lock_location_is_unavailable_not_busy() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("a-file");
        std::fs::write(&file, b"x").expect("write");
        // A path *under a regular file* can be neither created nor opened.
        let outcome = SingleInstance::acquire_at(&file.join("tray.lock"));
        assert!(matches!(outcome, Acquire::Unavailable(_)));
    }

    #[test]
    fn a_symlink_at_the_lock_path_is_unavailable_even_when_it_points_at_a_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let target = dir.path().join("target");
        std::fs::write(&target, b"x").expect("write");
        let link = dir.path().join("tray.lock");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert!(matches!(
            SingleInstance::acquire_at(&link),
            Acquire::Unavailable(_)
        ));
    }

    #[test]
    fn a_fifo_at_the_lock_path_is_unavailable_and_never_blocks() {
        let dir = tempfile::tempdir().expect("temp dir");
        let fifo = dir.path().join("tray.lock");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo runs");
        assert!(made.success());
        // Opening a FIFO for writing blocks without a reader: run it on a
        // thread so a regression fails by timeout instead of hanging the suite.
        // (A blocked thread is leaked on failure; the process ends anyway.)
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(matches!(
                SingleInstance::acquire_at(&fifo),
                Acquire::Unavailable(_)
            ));
        });
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(5)),
            Ok(true),
            "a FIFO must be refused before it is opened"
        );
    }

    #[test]
    fn a_directory_at_the_lock_path_is_unavailable() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("tray.lock");
        std::fs::create_dir(&path).expect("mkdir");
        assert!(matches!(
            SingleInstance::acquire_at(&path),
            Acquire::Unavailable(_)
        ));
    }

    #[test]
    fn our_own_regular_lock_file_is_held_and_reacquirable_after_release() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("tray.lock");
        std::fs::write(&path, b"").expect("pre-existing own file");
        let first = SingleInstance::acquire_at(&path);
        assert!(matches!(first, Acquire::Held(_)));
        drop(first);
        assert!(matches!(
            SingleInstance::acquire_at(&path),
            Acquire::Held(_)
        ));
    }

    #[test]
    fn an_opened_lock_file_must_be_regular_and_owned_by_the_caller() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("f");
        std::fs::write(&file, b"").expect("write");
        let own = rustix::process::getuid().as_raw();
        let file_meta = std::fs::metadata(&file).expect("meta");
        assert!(lock_file_verdict(&file_meta, own).is_ok());
        // Another uid (the opener is never root-owned here): refused.
        assert!(lock_file_verdict(&file_meta, own.wrapping_add(1)).is_err());
        // A directory is never a lock file, whoever owns it.
        let dir_meta = std::fs::metadata(dir.path()).expect("meta");
        assert!(lock_file_verdict(&dir_meta, own).is_err());
    }

    // --- Popover protocol ---------------------------------------------------

    #[test]
    fn every_served_asset_carries_the_frame_blocking_csp() {
        for path in ["/", "/index.html", "/popover.css", "/popover.js"] {
            let response = asset_response(path);
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let csp = response
                .headers()
                .get("Content-Security-Policy")
                .unwrap_or_else(|| panic!("{path}: no CSP"))
                .to_str()
                .expect("ascii");
            assert_eq!(csp, POPOVER_CSP, "{path}");
        }
    }

    #[test]
    fn the_csp_blocks_embedding_and_nothing_else() {
        for directive in [
            "frame-src 'none'",
            "child-src 'none'",
            "object-src 'none'",
            "frame-ancestors 'none'",
        ] {
            assert!(POPOVER_CSP.contains(directive), "{directive}");
        }
        for open in [
            "default-src",
            "script-src",
            "style-src",
            "img-src",
            "connect-src",
        ] {
            assert!(
                !POPOVER_CSP.contains(open),
                "{open} would restrict the page"
            );
        }
    }

    #[test]
    fn an_unknown_path_is_still_a_404() {
        for path in ["/missing", "/../etc/passwd", "/popover.js/", ""] {
            assert_eq!(
                asset_response(path).status(),
                StatusCode::NOT_FOUND,
                "{path:?}"
            );
        }
    }

    // --- IPC body budget -----------------------------------------------------

    #[test]
    fn an_ipc_body_at_the_limit_is_accepted_and_one_byte_more_is_not() {
        assert!(ipc_body_within_limit(0));
        assert!(ipc_body_within_limit(MAX_IPC_BODY_BYTES));
        assert!(!ipc_body_within_limit(MAX_IPC_BODY_BYTES + 1));
        // The largest PNG the image path allows still fits, as base64.
        assert!(ipc_body_within_limit(16 * 1024 * 1024 / 3 * 4 + 4096));
    }

    // --- Static sanity ----------------------------------------------------

    #[test]
    fn the_popover_url_is_on_the_origin_the_bridge_accepts() {
        assert!(ipc_allowed(POPOVER_URL));
        assert_eq!(navigation_decision(POPOVER_URL), Navigation::Allow);
    }
}
