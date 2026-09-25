//! NotifyIcon + WebView2 popover. Windows-only.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use serde_json::Value;
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::monitor::MonitorHandle;
use tao::platform::windows::{MonitorHandleExtWindows, WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Window, WindowBuilder};
use tray_icon::menu::{CheckMenuItem, ContextMenu, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, Rect, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND,
};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO};
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentProcessId};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, SM_CXSMICON, WindowFromPoint,
};
use wry::http::{Request, Response, StatusCode, header::CONTENT_TYPE};
use wry::{WebView, WebViewBuilder};

use super::blur::{self, Blur, Foreground, PressHide, Verdict};
use super::hotkey::{self, HotkeyBinding};
use super::icon::{Ink, Severity, apply_ink, tray_icon_rgba};
use super::payload::{
    HostFacts, SharedFacts, facts_snapshot, host_payload, with_facts, worst_severity, wrap_report,
};
use super::placement::{self, Area, Insets};
use super::updates::Updates;
use super::{now_ms, startup, taskbar_theme, tui_launch, update_flow};
use crate::config::{Config, UpdateMode};
use crate::update::{current_os, sweep_old};

// Emitted by `windows/popover` (`npm run build` / `build.rs` on Windows).
const INDEX_HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/popover/index.html"));
const POPOVER_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/popover/popover.css"));
const POPOVER_JS: &str = include_str!(concat!(env!("OUT_DIR"), "/popover/popover.js"));

/// Compact fixed width in logical px. The previous 320 px became visually
/// oversized on scaled Windows displays.
const WINDOW_WIDTH: f64 = 300.0;
/// Initial height only: the web content drives it afterwards via the
/// `resize` IPC command.
const WINDOW_HEIGHT: f64 = 420.0;
/// Smallest height a `resize` request can shrink the popover to.
/// Sized so the footer Options menu (nine rows, opens upward) fits without
/// Radix scrolling the list on short screens like Customize / provider detail.
const MIN_POPOVER_HEIGHT: f64 = 360.0;
/// Height the popover leaves free in the work area: the margin on the edge away from the
/// taskbar. The edge by the taskbar has none.
const WORK_AREA_MARGIN: f64 = 8.0;
/// Used when no monitor can be resolved at all.
const FALLBACK_WORK_AREA_HEIGHT: f64 = 800.0;
/// Gap between the popover and the tray icon or a work-area edge, in physical pixels. The edge
/// by the taskbar gets none (see [`Insets::by_taskbar`]).
const POPOVER_MARGIN: i32 = 8;
/// WebView2 background per theme (opaque; WebView2 ignores translucency).
const LIGHT_BACKGROUND: (u8, u8, u8, u8) = (255, 255, 255, 255);
const DARK_BACKGROUND: (u8, u8, u8, u8) = (30, 30, 30, 255);
/// The page ignores clicks this long after opening, so a stray mouse-up cannot hit the ⋮.
const CLICK_LOCK_MS: u64 = 400;
/// How often the outside-press watch looks at the mouse while the popover has no focus.
const PRESS_POLL: Duration = Duration::from_millis(30);

/// Set on the process an update relaunches, so it waits for the old one to
/// release the single-instance mutex instead of quitting at once.
const RELAUNCH_ENV: &str = "AIUB_TRAY_RELAUNCH";
/// How long a relaunched process keeps retrying the mutex.
const RELAUNCH_WAIT: Duration = Duration::from_secs(10);

/// Shown on the NotifyIcon when WebView2 is missing (older Windows 10).
/// The popover cannot open; the icon and right-click menu still work.
const WEBVIEW2_MISSING: &str = "Install the WebView2 Evergreen Runtime to open the popover.";

enum UserEvent {
    Tray(TrayIconEvent),
    Menu(MenuEvent),
    Ipc(String),
    Report(Value),
    /// One refreshed entry (Refresh <provider>), merged into the last report.
    Entry(Value),
    /// The shared facts changed (shortcut, update state); re-stamp the payload.
    Facts,
    /// The mouse went down outside the open popover; the `session` it was seen in, so a
    /// press from an earlier opening is ignored.
    OutsidePress {
        session: u64,
        x: i32,
        y: i32,
    },
    /// The global shortcut fired.
    Hotkey,
    /// A verified update is in place; start it and quit.
    Restart(PathBuf),
    /// Windows switched between light and dark; recolor the tray glyph.
    TaskbarTheme,
}

enum WorkerCmd {
    Refresh,
    RefreshEntry(String),
    Detect,
    CheckUpdate { manual: bool },
    InstallUpdate,
    SnoozeUpdate,
    SetUpdates(UpdateMode),
    Shutdown,
}

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
            Self::Light => LIGHT_BACKGROUND,
            Self::Dark => DARK_BACKGROUND,
        }
    }
}

struct MenuItems {
    refresh: MenuItem,
    detect: MenuItem,
    open_tui: MenuItem,
    startup: CheckMenuItem,
    quit: MenuItem,
}

struct TrayState {
    window: Window,
    webview: Option<WebView>,
    tray: TrayIcon,
    menu: MenuItems,
    context_menu: Menu,
    worker: mpsc::Sender<WorkerCmd>,
    proxy: EventLoopProxy<UserEvent>,
    payload: Value,
    js_ready: bool,
    popover_open: bool,
    /// Blurs before this instant are the ones our own show/focus calls
    /// produce and are ignored; a stale flag would swallow the user's first
    /// click outside, so this is a deadline rather than a boolean.
    blur_guard_until: Option<Instant>,
    /// The last hide caused by a press outside, and when, so the tray icon's mouse-up that
    /// follows a press on the icon closes the popover instead of reopening it.
    last_press_hide: Option<(Instant, PressHide)>,
    /// Bumped on every show and hide: the outside-press watch of an earlier opening sees it
    /// change and stops.
    popover_session: Arc<AtomicU64>,
    /// Tray rect from the last `show_popover`, reused when a `resize`
    /// re-anchors the visible popover above the NotifyIcon.
    last_tray_rect: Option<Rect>,
    /// Last theme the page reported, so a rebuilt webview starts matching.
    theme: Theme,
    /// Tray glyph color for the current taskbar.
    ink: Ink,
    facts: SharedFacts,
    /// `None` when the hotkey manager could not be created; the Settings
    /// row then reports every attempt as failed.
    hotkey: Option<HotkeyBinding>,
}

pub fn run() -> i32 {
    let relaunched = std::env::var_os(RELAUNCH_ENV).is_some();
    let Some(_mutex) = SingleInstance::acquire_waiting(relaunched) else {
        return 0;
    };
    match run_loop() {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

fn run_loop() -> Result<(), String> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

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
        .with_title("AI Usage")
        .with_inner_size(LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT))
        .with_visible(false)
        .with_decorations(false)
        .with_always_on_top(true)
        .with_resizable(false)
        .with_focused(false)
        .with_skip_taskbar(true)
        .build(&event_loop)
        .map_err(|error| error.to_string())?;
    round_corners(&window);

    let config = Config::load().unwrap_or_default();
    let facts: SharedFacts = Arc::new(Mutex::new(host_facts(&config)));

    // Created here because the crate needs the thread that runs the win32
    // message loop; tao's loop is this one.
    let mut hotkey_binding = HotkeyBinding::new().ok();
    {
        let proxy = proxy.clone();
        hotkey::install_press_handler(move |_| {
            let _ = proxy.send_event(UserEvent::Hotkey);
        });
    }
    if let Some(configured) = config.tray.shortcut.as_deref() {
        let outcome = bind_shortcut(hotkey_binding.as_mut(), configured);
        with_facts(&facts, |f| apply_shortcut_outcome(f, outcome));
    }
    // Leftovers from the swap that put this exe in place.
    if let Ok(dir) = update_flow::install_dir() {
        let _ = sweep_old(&dir, current_os());
    }

    let (cmd_tx, cmd_rx) = mpsc::channel();
    spawn_worker(proxy.clone(), cmd_rx, facts.clone());
    let _ = cmd_tx.send(WorkerCmd::Refresh);

    let empty = wrap_report("{}", &facts_snapshot(&facts), now_ms(), None);
    let menu = build_menu(startup::is_enabled());
    let context_menu = make_menu(&menu);
    let ink = taskbar_theme::ink();
    let tray = build_tray(&empty, ink)?;
    {
        let proxy = proxy.clone();
        taskbar_theme::watch(move || {
            let _ = proxy.send_event(UserEvent::TaskbarTheme);
        });
    }
    // SAFETY: the tray hwnd lives as long as `tray`. Attach so MenuEvent
    // still fires without letting tray-icon auto-popup on mouse-down
    // (Windows can emit a phantom right-down with a left click).
    unsafe {
        context_menu.attach_menu_subclass_for_hwnd(tray.window_handle() as isize);
    }

    let theme = Theme::Light;
    let webview = build_webview(&window, proxy.clone(), theme).ok();
    if webview.is_none() {
        let _ = tray.set_tooltip(Some(WEBVIEW2_MISSING));
    }

    let mut state = TrayState {
        window,
        webview,
        tray,
        menu,
        context_menu,
        worker: cmd_tx,
        proxy: proxy.clone(),
        payload: empty,
        js_ready: false,
        popover_open: false,
        blur_guard_until: None,
        last_press_hide: None,
        popover_session: Arc::new(AtomicU64::new(0)),
        last_tray_rect: None,
        theme,
        ink,
        facts,
        hotkey: hotkey_binding,
    };

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UserEvent::Tray(tray_event)) => {
                handle_tray(&mut state, tray_event);
            }
            Event::UserEvent(UserEvent::Menu(menu_event)) => {
                handle_menu(&mut state, &menu_event, control_flow);
            }
            Event::UserEvent(UserEvent::Ipc(body)) => {
                handle_ipc(&mut state, &body, control_flow);
            }
            Event::UserEvent(UserEvent::Report(payload)) => {
                apply_payload(&mut state, payload);
            }
            Event::UserEvent(UserEvent::Entry(entry)) => {
                apply_entry(&mut state, entry);
            }
            Event::UserEvent(UserEvent::Facts) => {
                apply_facts(&mut state);
            }
            Event::UserEvent(UserEvent::Hotkey) => {
                toggle_popover_from_keyboard(&mut state);
            }
            Event::UserEvent(UserEvent::TaskbarTheme) => {
                let ink = taskbar_theme::ink();
                if ink != state.ink {
                    state.ink = ink;
                    refresh_icon(&mut state);
                }
            }
            Event::UserEvent(UserEvent::Restart(exe)) => {
                relaunch(&exe);
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(UserEvent::OutsidePress { session, x, y }) => {
                if state.popover_open && session == state.popover_session.load(Ordering::SeqCst) {
                    trace(&format!(
                        "press outside → hide; foreground = {}",
                        foreground_window_label()
                    ));
                    hide_popover(&mut state);
                    state.last_press_hide = Some((Instant::now(), PressHide { x, y }));
                }
            }
            Event::WindowEvent {
                event: WindowEvent::Focused(false),
                ..
            } => {
                if state.popover_open {
                    let facts = blur_facts(blur_guarded(&state));
                    match blur::verdict(facts) {
                        Verdict::Hide => {
                            trace(&format!(
                                "blur → hide; foreground = {}",
                                foreground_window_label()
                            ));
                            hide_popover(&mut state);
                            if facts.button_down {
                                let (x, y) = cursor_position();
                                state.last_press_hide = Some((Instant::now(), PressHide { x, y }));
                            }
                        }
                        Verdict::Keep(reason) => {
                            trace(&format!(
                                "blur kept open ({reason}); foreground = {}",
                                foreground_window_label()
                            ));
                        }
                    }
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                hide_popover(&mut state);
            }
            Event::LoopDestroyed => {
                let _ = state.worker.send(WorkerCmd::Shutdown);
            }
            _ => {}
        }
    });
}

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
            // turn on the vendors whose credentials already exist locally, so the
            // very first report already carries them.
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
                // Commands that do not need a whole new report are served
                // until the poll deadline, so a Refresh <provider> does not
                // postpone the next full report.
                // Read per cycle so a `set-refresh` applies on the next one.
                let deadline =
                    Instant::now() + Duration::from_secs(facts_snapshot(&facts).refresh_secs);
                loop {
                    let wait = deadline.saturating_duration_since(Instant::now());
                    match rx.recv_timeout(wait) {
                        Ok(WorkerCmd::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Ok(WorkerCmd::Detect) => {
                            run_detection(true);
                            break;
                        }
                        Ok(WorkerCmd::RefreshEntry(id)) => {
                            rt.block_on(push_entry(&proxy, &id));
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
/// every vendor (Options → Detect Providers); otherwise only vendors this
/// install has not seen before are probed, so a vendor the user turned off
/// stays off.
fn run_detection(force: bool) {
    if let Ok(state_path) = crate::detect::default_state_path() {
        let _ = crate::detect::run_once(None, &state_path, force);
    }
}

/// Facts about this process at startup; the shortcut and update fields are
/// filled in as the host learns them.
fn host_facts(config: &Config) -> HostFacts {
    let mut facts = HostFacts::new(env!("CARGO_PKG_VERSION"), startup::is_enabled());
    facts.updates = config.tray.updates().as_str().into();
    facts.refresh_secs = config.tray.refresh_minutes() * 60;
    facts
}

async fn push_report(proxy: &EventLoopProxy<UserEvent>, facts: &SharedFacts) {
    let mut snapshot = facts_snapshot(facts);
    snapshot.startup_enabled = startup::is_enabled();
    let now = now_ms();
    let payload = match crate::report::collect_json().await {
        Ok(json) => wrap_report(&json, &snapshot, now, None),
        Err(error) => wrap_report("{}", &snapshot, now, Some(&error)),
    };
    let _ = proxy.send_event(UserEvent::Report(payload));
}

/// Refresh one provider (row menu → Refresh). A failure is folded into the
/// entry itself so the card shows it; the rest of the report is untouched.
async fn push_entry(proxy: &EventLoopProxy<UserEvent>, id: &str) {
    let entry = match crate::report::collect_entry_json(id).await {
        Ok(json) => serde_json::from_str::<Value>(&json)
            .ok()
            .and_then(|v| v.get("entries")?.as_array()?.first().cloned()),
        Err(error) => Some(serde_json::json!({
            "id": id,
            "status": "error",
            "error": crate::display::sanitize_untrusted_field(&error),
            "sections": [],
        })),
    };
    if let Some(entry) = entry {
        let _ = proxy.send_event(UserEvent::Entry(entry));
    }
}

fn apply_payload(state: &mut TrayState, payload: Value) {
    trace(&format!(
        "report arrived (popover_open = {}, focused = {})",
        state.popover_open,
        state.window.is_focused()
    ));
    state.payload = payload;
    refresh_icon(state);
    // No hover tip: the popover is the readout. The one exception names the
    // missing WebView2 runtime, because without it there is no popover.
    if state.webview.is_none() {
        let _ = state.tray.set_tooltip(Some(WEBVIEW2_MISSING));
    }
    stamp_facts(state);
    if state.js_ready {
        push_to_webview(state);
    }
}

/// Copy the shared facts onto the payload in place, so a shortcut or update
/// change shows up without waiting for the next report.
fn stamp_facts(state: &mut TrayState) {
    let facts = facts_snapshot(&state.facts);
    let stamped = wrap_report("{}", &facts, 0, None);
    let Some(obj) = state.payload.as_object_mut() else {
        return;
    };
    for key in [
        "shortcut",
        "shortcut_error",
        "updates",
        "update",
        "update_checked_at",
        "refresh_minutes",
        "repository",
        "version",
    ] {
        obj.insert(key.into(), stamped[key].clone());
    }
}

fn apply_facts(state: &mut TrayState) {
    stamp_facts(state);
    if state.js_ready {
        push_to_webview(state);
    }
}

fn apply_entry(state: &mut TrayState, entry: Value) {
    let Some(id) = entry.get("id").and_then(Value::as_str).map(str::to_owned) else {
        return;
    };
    let Some(entries) = state
        .payload
        .get_mut("entries")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    match entries
        .iter_mut()
        .find(|e| e.get("id").and_then(Value::as_str) == Some(id.as_str()))
    {
        Some(slot) => *slot = entry,
        None => entries.push(entry),
    }
    refresh_icon(state);
    if state.js_ready {
        push_to_webview(state);
    }
}

/// Shortcut press: toggle.
fn toggle_popover_from_keyboard(state: &mut TrayState) {
    if state.popover_open {
        hide_popover(state);
    } else {
        let rect = state.last_tray_rect;
        show_popover(state, rect);
    }
}

fn relaunch(exe: &std::path::Path) {
    let _ = std::process::Command::new(exe)
        .env(RELAUNCH_ENV, "1")
        .spawn();
}

enum ShortcutOutcome {
    Bound(String),
    Cleared,
    Refused { attempted: String, reason: String },
}

/// Normalize and register `text`; an empty string unregisters.
fn bind_shortcut(binding: Option<&mut HotkeyBinding>, text: &str) -> ShortcutOutcome {
    if text.trim().is_empty() {
        if let Some(binding) = binding {
            let _ = binding.apply(None);
        }
        return ShortcutOutcome::Cleared;
    }
    let normalized = match hotkey::normalize(text) {
        Ok(n) => n,
        Err(reason) => {
            return ShortcutOutcome::Refused {
                attempted: String::new(),
                reason,
            };
        }
    };
    let Some(binding) = binding else {
        return ShortcutOutcome::Refused {
            attempted: normalized.canonical,
            reason: "Global shortcuts are unavailable in this session.".into(),
        };
    };
    match binding.apply(Some(&normalized.canonical)) {
        Ok(()) => ShortcutOutcome::Bound(normalized.canonical),
        Err(reason) => ShortcutOutcome::Refused {
            attempted: normalized.canonical,
            reason,
        },
    }
}

fn apply_shortcut_outcome(facts: &mut HostFacts, outcome: ShortcutOutcome) {
    match outcome {
        ShortcutOutcome::Bound(canonical) => {
            facts.shortcut = canonical;
            facts.shortcut_error.clear();
        }
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
    let outcome = bind_shortcut(state.hotkey.as_mut(), value);
    let persisted = match &outcome {
        ShortcutOutcome::Bound(canonical) => Some(Some(canonical.clone())),
        ShortcutOutcome::Cleared => Some(None),
        ShortcutOutcome::Refused { .. } => None,
    };
    if let Some(value) = persisted
        && let Some(path) = config_path()
    {
        let _ = crate::config::set_tray_value(&path, "shortcut", value.map(Into::into));
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
    // Start a cycle now so the footer's countdown and the worker's deadline
    // follow the new cadence at once instead of after the old one expires.
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

fn handle_tray(state: &mut TrayState, event: TrayIconEvent) {
    match event {
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            rect,
            ..
        } => {
            let press = state.last_press_hide.take();
            let elapsed_ms = press.map_or(u128::MAX, |(at, _)| at.elapsed().as_millis());
            if state.popover_open {
                hide_popover(state);
            } else if blur::is_toggle_close(press.map(|(_, p)| p), elapsed_ms, icon_area(rect)) {
                trace("icon click after the press that closed the popover → stay closed");
            } else {
                show_popover(state, Some(rect));
            }
        }
        TrayIconEvent::Click {
            button: MouseButton::Right,
            button_state: MouseButtonState::Up,
            ..
        } => {
            hide_popover(state);
            let hwnd = state.tray.window_handle() as isize;
            // SAFETY: hwnd is the tray message window, valid while `tray` lives.
            // None uses the cursor, which is still over the NotifyIcon.
            unsafe {
                let _ = state.context_menu.show_context_menu_for_hwnd(hwnd, None);
            }
        }
        _ => {}
    }
}

fn handle_menu(state: &mut TrayState, event: &MenuEvent, control_flow: &mut ControlFlow) {
    if event.id == state.menu.refresh.id() {
        let _ = state.worker.send(WorkerCmd::Refresh);
    } else if event.id == state.menu.detect.id() {
        let _ = state.worker.send(WorkerCmd::Detect);
    } else if event.id == state.menu.open_tui.id() {
        tui_launch::open();
    } else if event.id == state.menu.startup.id() {
        toggle_startup(state);
    } else if event.id == state.menu.quit.id() {
        *control_flow = ControlFlow::Exit;
    }
}

fn handle_ipc(state: &mut TrayState, body: &str, control_flow: &mut ControlFlow) {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return;
    };
    let cmd = value.get("cmd").and_then(Value::as_str).unwrap_or("");
    match cmd {
        "ready" => {
            state.js_ready = true;
            push_to_webview(state);
        }
        "detect" => {
            let _ = state.worker.send(WorkerCmd::Detect);
        }
        "refresh" => {
            let _ = state.worker.send(WorkerCmd::Refresh);
        }
        "open-tui" => tui_launch::open(),
        "close" => {
            trace("ipc close → hide");
            hide_popover(state);
        }
        "quit" => *control_flow = ControlFlow::Exit,
        "toggle-startup" => toggle_startup(state),
        "resize" => handle_resize(state, &value),
        "refresh-entry" => {
            if let Some(id) = value.get("id").and_then(Value::as_str) {
                let _ = state.worker.send(WorkerCmd::RefreshEntry(id.to_owned()));
            }
        }
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
        "strip" => {}
        "open-url" => {
            if let Some(url) = value.get("url").and_then(Value::as_str) {
                super::browse::open(url);
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
        _ => {}
    }
}

/// `{"cmd":"resize","height":<logical px>,"theme":"light"|"dark"}`.
/// `theme` is optional; a missing or malformed `height` is ignored.
fn handle_resize(state: &mut TrayState, value: &Value) {
    if let Some(theme) = value
        .get("theme")
        .and_then(Value::as_str)
        .and_then(Theme::parse)
    {
        apply_theme(state, theme);
    }
    let Some(requested) = value.get("height").and_then(Value::as_f64) else {
        return;
    };
    if !requested.is_finite() || requested <= 0.0 {
        return;
    }
    let target = clamp_popover_height(requested, work_area_height(&state.window));
    state
        .window
        .set_inner_size(LogicalSize::new(WINDOW_WIDTH, target));
    if state.popover_open {
        position_window(&state.window, state.last_tray_rect);
    }
}

fn apply_theme(state: &mut TrayState, theme: Theme) {
    if state.theme == theme {
        return;
    }
    state.theme = theme;
    if let Some(webview) = state.webview.as_ref() {
        let _ = webview.set_background_color(theme.background());
    }
}

/// Height the popover may grow to for `requested` logical px: rounded, never
/// below [`MIN_POPOVER_HEIGHT`], never past the work area minus its margin.
pub(crate) fn clamp_popover_height(requested: f64, work_area_height: f64) -> f64 {
    let max = (work_area_height - WORK_AREA_MARGIN).max(MIN_POPOVER_HEIGHT);
    requested.round().clamp(MIN_POPOVER_HEIGHT, max)
}

/// Logical inner height the popover may take on the monitor it sits on (primary as fallback):
/// that monitor's work area, so it never runs under the taskbar, less the window frame.
fn work_area_height(window: &Window) -> f64 {
    let Some(monitor) = window
        .current_monitor()
        .or_else(|| window.primary_monitor())
    else {
        return FALLBACK_WORK_AREA_HEIGHT;
    };
    let (_, work) = monitor_areas(&monitor);
    let frame = window.outer_size().height as i32 - window.inner_size().height as i32;
    placement::max_inner_height(work, frame, monitor.scale_factor())
}

/// A monitor's full rectangle and its work area (`rcWork`: less the taskbar and docked app
/// bars), in physical pixels. tao reports only the full rectangle; if the Win32 call fails the
/// work area falls back to it.
fn monitor_areas(monitor: &MonitorHandle) -> (Area, Area) {
    let origin = monitor.position();
    let size = monitor.size();
    let full = Area {
        x: origin.x,
        y: origin.y,
        width: size.width as i32,
        height: size.height as i32,
    };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: the handle comes from a live tao MonitorHandle, and `info` is a properly sized
    // MONITORINFO the call only writes into.
    let ok = unsafe { GetMonitorInfoW(monitor.hmonitor() as _, &mut info) } != 0;
    if !ok {
        return (full, full);
    }
    let rect = |r: windows_sys::Win32::Foundation::RECT| Area {
        x: r.left,
        y: r.top,
        width: r.right - r.left,
        height: r.bottom - r.top,
    };
    (rect(info.rcMonitor), rect(info.rcWork))
}

/// Ask DWM for Windows 11 rounded corners. Windows 10 rejects the attribute;
/// the popover then stays square, which is fine.
fn round_corners(window: &Window) {
    let hwnd = window.hwnd() as HWND;
    let preference = DWMWCP_ROUND;
    // SAFETY: hwnd is the live popover window; `preference` outlives the call
    // and its size is passed as `cbattribute`.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            std::ptr::from_ref(&preference).cast(),
            std::mem::size_of_val(&preference) as u32,
        );
    }
}

fn toggle_startup(state: &mut TrayState) {
    let next = !startup::is_enabled();
    if startup::set_enabled(next).is_ok() {
        state.menu.startup.set_checked(next);
        if let Some(obj) = state.payload.as_object_mut() {
            obj.insert("startup_enabled".into(), Value::Bool(next));
        }
        if state.js_ready {
            push_to_webview(state);
        }
    } else {
        state.menu.startup.set_checked(startup::is_enabled());
    }
}

fn show_popover(state: &mut TrayState, tray_rect: Option<Rect>) {
    state.last_tray_rect = tray_rect;
    state.last_press_hide = None;
    position_window(&state.window, tray_rect);
    guard_blur(state);
    state.window.set_visible(true);
    state.popover_open = true;
    // The page ignores clicks for a moment, so a stray mouse-up cannot hit the ⋮ in the footer.
    if let Some(webview) = state.webview.as_ref() {
        let _ = webview.evaluate_script(&format!(
            "window.__AIUB_LOCKCLICKS__ && window.__AIUB_LOCKCLICKS__({CLICK_LOCK_MS})"
        ));
        let _ = webview.evaluate_script("window.__AIUB_VISIBLE__ && window.__AIUB_VISIBLE__(true)");
    }
    if state.js_ready {
        push_to_webview(state);
    }
    // Focus now, while the click on the tray icon still lets this process take the
    // foreground; focusing later failed once the user had clicked elsewhere.
    state.window.set_focus();
    let session = state.popover_session.fetch_add(1, Ordering::SeqCst) + 1;
    watch_outside_press(state, session);
}

/// Closes the popover on a press outside it for as long as it is open. A blur alone is not
/// enough: a popover Windows refused to focus has no focus to lose, and once focus moves into
/// the WebView2 child the window's own blur has already fired (kept, as focus stayed here), so a
/// later click elsewhere can bring no second one. Either way the popover stayed open.
fn watch_outside_press(state: &TrayState, session: u64) {
    let current = state.popover_session.clone();
    let proxy = state.proxy.clone();
    let hwnd = state.window.hwnd();
    std::thread::spawn(move || {
        let mut was_down = mouse_button_down();
        loop {
            std::thread::sleep(PRESS_POLL);
            if current.load(Ordering::SeqCst) != session {
                return;
            }
            let down = mouse_button_down();
            if down && !was_down {
                let (x, y) = cursor_position();
                if !window_contains(hwnd, x, y) {
                    let _ = proxy.send_event(UserEvent::OutsidePress { session, x, y });
                    return;
                }
            }
            was_down = down;
        }
    });
}

fn mouse_button_down() -> bool {
    // SAFETY: GetAsyncKeyState has no preconditions.
    unsafe {
        (GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000 != 0)
            || (GetAsyncKeyState(VK_RBUTTON as i32) as u16 & 0x8000 != 0)
    }
}

fn cursor_position() -> (i32, i32) {
    let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    // SAFETY: `point` is a valid out-pointer for the call.
    unsafe { GetCursorPos(&mut point) };
    (point.x, point.y)
}

fn window_contains(hwnd: isize, x: i32, y: i32) -> bool {
    let mut rect = windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: `hwnd` is the popover, alive while its session lasts; `rect` is a valid
    // out-pointer. A failed call leaves an empty rect, which contains nothing.
    unsafe { GetWindowRect(hwnd as HWND, &mut rect) };
    x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
}

/// A tray icon rect in physical pixels, for the placement and toggle decisions.
fn icon_area(rect: Rect) -> Area {
    Area {
        x: rect.position.x.round() as i32,
        y: rect.position.y.round() as i32,
        width: rect.size.width as i32,
        height: rect.size.height as i32,
    }
}

/// Opt-in diagnostics: set `AIUB_TRAY_TRACE=1` and every hide, blur and
/// report lands in `%TEMP%i-usagebar-tray-trace.log` with the window
/// that held the foreground. Off, this is a single atomic load.
fn trace(message: &str) {
    use std::io::Write;
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("AIUB_TRAY_TRACE").is_some()) {
        return;
    }
    let path = std::env::temp_dir().join("ai-usagebar-tray-trace.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{} {message}", now_ms());
    }
}

/// "<hwnd> <class> '<title>'" of the foreground window, plus the cursor's
/// position, the window under it and whether a mouse button is down — enough
/// to tell a click outside from a window that activated itself.
fn foreground_window_label() -> String {
    // SAFETY: plain user32 queries; every buffer outlives its call.
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut title = [0u16; 128];
        let n = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32).max(0) as usize;
        let title = String::from_utf16_lossy(&title[..n]);
        let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        GetCursorPos(&mut point);
        let under = WindowFromPoint(point);
        let buttons = mouse_button_down();
        format!(
            "{hwnd:?} {} '{}'; cursor ({}, {}) over {under:?} {}; button down = {buttons}",
            window_class(hwnd),
            crate::display::sanitize_untrusted_line(&title),
            point.x,
            point.y,
            window_class(under),
        )
    }
}

/// Windows shell surfaces that activate themselves now and then (a badge
/// refresh, an overflow relayout) without the user touching them. The last one
/// is the Windows 11 tray overflow flyout.
const SHELL_CLASSES: [&str; 5] = [
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "TrayNotifyWnd",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
];

/// What a blur looks like from here: who took the foreground and whether a
/// mouse button is down. [`blur::verdict`] decides what it means.
fn blur_facts(guarded: bool) -> Blur {
    // SAFETY: plain user32 queries with valid out-pointers.
    let foreground = unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            Foreground::Nobody
        } else {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == GetCurrentProcessId() {
                Foreground::Ours
            } else if SHELL_CLASSES.contains(&window_class(hwnd).as_str()) {
                Foreground::Shell
            } else {
                Foreground::Other
            }
        }
    };
    Blur {
        button_down: mouse_button_down(),
        foreground,
        guarded,
    }
}

fn window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    // SAFETY: the buffer outlives the call; a null hwnd yields 0 chars.
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// Showing or focusing the window can bounce a `Focused(false)` through the
/// loop; ignore blurs for a moment instead of latching a flag.
const BLUR_GUARD: Duration = Duration::from_millis(400);

fn guard_blur(state: &mut TrayState) {
    state.blur_guard_until = Some(Instant::now() + BLUR_GUARD);
}

fn blur_guarded(state: &TrayState) -> bool {
    state
        .blur_guard_until
        .is_some_and(|until| Instant::now() < until)
}

fn hide_popover(state: &mut TrayState) {
    state.window.set_visible(false);
    state.popover_open = false;
    state.popover_session.fetch_add(1, Ordering::SeqCst);
    // Closing resets navigation (OpenUsage: scroll to top, Customize / Settings
    // close) so the next open lands on the dashboard.
    if let Some(webview) = state.webview.as_ref() {
        let _ =
            webview.evaluate_script("window.__AIUB_VISIBLE__ && window.__AIUB_VISIBLE__(false)");
    }
}

fn position_window(window: &Window, tray_rect: Option<Rect>) {
    let size = window.outer_size();
    let (width, height) = (size.width as i32, size.height as i32);
    let (x, y) = if let Some(rect) = tray_rect {
        // Centered on the tray icon, on the side away from whichever edge the taskbar is on.
        let icon = icon_area(rect);
        let center_x = rect.position.x + f64::from(rect.size.width) / 2.0;
        let center_y = rect.position.y + f64::from(rect.size.height) / 2.0;
        match window
            .monitor_from_point(center_x, center_y)
            .or_else(|| window.primary_monitor())
        {
            Some(monitor) => {
                let (full, work) = monitor_areas(&monitor);
                placement::beside_icon(full, work, icon, width, height, POPOVER_MARGIN)
            }
            None => (
                (icon.x + icon.width / 2 - width / 2).max(POPOVER_MARGIN),
                (icon.y - height - POPOVER_MARGIN).max(POPOVER_MARGIN),
            ),
        }
    } else if let Some(monitor) = window.primary_monitor() {
        // The global shortcut: no icon to hang from, so the corner by the taskbar.
        let (full, work) = monitor_areas(&monitor);
        let insets = Insets::by_taskbar(full, work, POPOVER_MARGIN);
        placement::corner_near_taskbar(full, work, width, height, insets)
    } else {
        (80, 80)
    };
    window.set_outer_position(PhysicalPosition::new(x, y));
}

fn build_menu(startup_enabled: bool) -> MenuItems {
    MenuItems {
        refresh: MenuItem::with_id("refresh", "Refresh", true, None),
        detect: MenuItem::with_id("detect", "Detect Providers", true, None),
        open_tui: MenuItem::with_id("open-tui", "Open TUI", true, None),
        startup: CheckMenuItem::with_id(
            "startup",
            "Start with Windows",
            true,
            startup_enabled,
            None,
        ),
        quit: MenuItem::with_id("quit", "Quit", true, None),
    }
}

fn make_menu(items: &MenuItems) -> Menu {
    let menu = Menu::new();
    let sep = PredefinedMenuItem::separator();
    let _ = menu.append_items(&[
        &items.refresh,
        &items.detect,
        &items.open_tui,
        &sep,
        &items.startup,
        &items.quit,
    ]);
    menu
}

fn build_tray(payload: &Value, ink: Ink) -> Result<TrayIcon, String> {
    let icon =
        icon_from_severity(worst_severity(payload), ink).map_err(|error| error.to_string())?;
    TrayIconBuilder::new()
        .with_icon(icon)
        .with_menu_on_left_click(false)
        .build()
        .map_err(|error| error.to_string())
}

/// The raster matching the shell's small-icon size for the current DPI
/// (`SM_CXSMICON`: 16 px at 100 %, 24 px at 150 %), so Windows draws it
/// without a second resample, in the ink that reads on the taskbar.
fn icon_from_severity(severity: Severity, ink: Ink) -> Result<Icon, tray_icon::BadIcon> {
    // SAFETY: GetSystemMetrics has no preconditions and touches no memory.
    let wanted = unsafe { GetSystemMetrics(SM_CXSMICON) };
    let wanted = u32::try_from(wanted).unwrap_or(16).max(16);
    let (mut rgba, size) = tray_icon_rgba(wanted, severity);
    apply_ink(&mut rgba, ink);
    Icon::from_rgba(rgba, size, size)
}

/// Redraw the tray glyph for the current report and taskbar.
fn refresh_icon(state: &mut TrayState) {
    if let Ok(icon) = icon_from_severity(worst_severity(&state.payload), state.ink) {
        let _ = state.tray.set_icon(Some(icon));
    }
}

fn build_webview(
    window: &Window,
    proxy: EventLoopProxy<UserEvent>,
    theme: Theme,
) -> Result<WebView, String> {
    WebViewBuilder::new()
        .with_custom_protocol("aiub".into(), move |_id, request| {
            protocol_response(request)
        })
        .with_url("http://aiub.localhost/index.html")
        .with_ipc_handler(move |request| {
            let body = request.body().clone();
            let _ = proxy.send_event(UserEvent::Ipc(body));
        })
        .with_background_color(theme.background())
        .build(window)
        .map_err(|error| error.to_string())
}

fn protocol_response(request: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = request.uri().path();
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
        .body(Cow::Borrowed(body))
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(body)))
}

struct SingleInstance(HANDLE);

impl SingleInstance {
    /// A relaunch after an update races the old process's exit; keep
    /// retrying for a bounded time instead of silently quitting.
    fn acquire_waiting(wait: bool) -> Option<Self> {
        if !wait {
            return Self::acquire();
        }
        let deadline = Instant::now() + RELAUNCH_WAIT;
        loop {
            if let Some(instance) = Self::acquire() {
                return Some(instance);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn acquire() -> Option<Self> {
        let name: Vec<u16> = "Local\\ai-usagebar-tray\0".encode_utf16().collect();
        // SAFETY: `name` is a NUL-terminated UTF-16 mutex name in the Local
        // namespace. A null security descriptor uses the default DACL.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            // Mutex API failed; do not block launch. Drop skips CloseHandle.
            return Some(Self(handle));
        }
        let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if exists {
            unsafe { CloseHandle(handle) };
            return None;
        }
        Some(Self(handle))
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        if !self.handle_is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

impl SingleInstance {
    fn handle_is_null(&self) -> bool {
        self.0.is_null()
    }
}

#[cfg(test)]
mod tests {
    use super::{MIN_POPOVER_HEIGHT, WORK_AREA_MARGIN, clamp_popover_height};

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
}
