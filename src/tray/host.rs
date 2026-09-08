//! NotifyIcon + WebView2 popover. Windows-only.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
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
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentProcessId};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindowTextW,
    GetWindowThreadProcessId, SM_CXSMICON, WindowFromPoint,
};
use wry::http::{Request, Response, StatusCode, header::CONTENT_TYPE};
use wry::{WebView, WebViewBuilder};

use super::hotkey::{self, HotkeyBinding};
use super::icon::{Severity, tray_icon_rgba};
use super::payload::{HostFacts, UpdateFact, host_payload, worst_severity, wrap_report};
use super::{startup, tui_launch, update_flow};
use crate::config::{Config, UpdateMode};
use crate::update::{CHECK_INTERVAL, Release, UpdateState, sweep_old};

// Emitted by `windows/popover` (`npm run build` / `build.rs` on Windows).
const INDEX_HTML: &str = include_str!("../../windows/popover/dist/index.html");
const POPOVER_CSS: &str = include_str!("../../windows/popover/dist/popover.css");
const POPOVER_JS: &str = include_str!("../../windows/popover/dist/popover.js");

/// Fixed popover width in logical px, matching the OpenUsage macOS panel.
const WINDOW_WIDTH: f64 = 320.0;
/// Initial height only: the web content drives it afterwards via the
/// `resize` IPC command.
const WINDOW_HEIGHT: f64 = 420.0;
/// Smallest height a `resize` request can shrink the popover to.
const MIN_POPOVER_HEIGHT: f64 = 120.0;
/// Breathing room kept between the popover and the monitor's edges.
const WORK_AREA_MARGIN: f64 = 16.0;
/// Used when no monitor can be resolved at all.
const FALLBACK_WORK_AREA_HEIGHT: f64 = 800.0;
/// WebView2 background per theme (opaque; WebView2 ignores translucency).
const LIGHT_BACKGROUND: (u8, u8, u8, u8) = (255, 255, 255, 255);
const DARK_BACKGROUND: (u8, u8, u8, u8) = (30, 30, 30, 255);
/// Absorb the mouse-up that opened the popover so it cannot hit the ⋮.
const CLICK_LOCK_MS: u64 = 400;

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
    FocusPopover,
    /// The global shortcut fired.
    Hotkey,
    /// A verified update is in place; start it and quit.
    Restart(PathBuf),
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

/// Host facts are owned jointly: the event-loop thread edits the shortcut, the
/// worker edits the update state, and every report snapshots the whole thing.
type SharedFacts = Arc<Mutex<HostFacts>>;

fn facts_snapshot(facts: &SharedFacts) -> HostFacts {
    facts.lock().map(|f| f.clone()).unwrap_or_default()
}

fn with_facts(facts: &SharedFacts, edit: impl FnOnce(&mut HostFacts)) {
    if let Ok(mut guard) = facts.lock() {
        edit(&mut guard);
    }
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
    /// Tray rect from the last `show_popover`, reused when a `resize`
    /// re-anchors the visible popover above the NotifyIcon.
    last_tray_rect: Option<Rect>,
    /// Last theme the page reported, so a rebuilt webview starts matching.
    theme: Theme,
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
        let _ = sweep_old(&dir);
    }

    let (cmd_tx, cmd_rx) = mpsc::channel();
    spawn_worker(proxy.clone(), cmd_rx, facts.clone());
    let _ = cmd_tx.send(WorkerCmd::Refresh);

    let empty = wrap_report("{}", &facts_snapshot(&facts), now_ms(), None);
    let menu = build_menu(startup::is_enabled());
    let context_menu = make_menu(&menu);
    let tray = build_tray(&empty)?;
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
        last_tray_rect: None,
        theme,
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
            Event::UserEvent(UserEvent::Restart(exe)) => {
                relaunch(&exe);
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(UserEvent::FocusPopover) => {
                if state.popover_open {
                    guard_blur(&mut state);
                    state.window.set_focus();
                }
            }
            Event::WindowEvent {
                event: WindowEvent::Focused(false),
                ..
            } => {
                if blur_guarded(&state) {
                    trace(&format!(
                        "blur ignored (guarded); foreground = {}",
                        foreground_window_label()
                    ));
                } else if state.popover_open {
                    match blur_verdict() {
                        BlurVerdict::Hide => {
                            trace(&format!(
                                "blur → hide; foreground = {}",
                                foreground_window_label()
                            ));
                            hide_popover(&mut state);
                        }
                        BlurVerdict::Keep(reason) => {
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
            let mut updates = Updates::new(facts.clone(), proxy.clone());
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
                        Ok(WorkerCmd::InstallUpdate) => {
                            if updates.pending.is_some() {
                                rt.block_on(updates.install());
                            } else {
                                rt.block_on(updates.check(true));
                            }
                        }
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

/// Worker-side update machinery: the hourly check, the snooze file and the
/// install. Every state change lands in the shared facts and is announced
/// with `UserEvent::Facts` so the popover re-renders at once.
struct Updates {
    client: Option<reqwest::Client>,
    facts: SharedFacts,
    pending: Option<Release>,
    proxy: EventLoopProxy<UserEvent>,
    state: UpdateState,
    state_path: Option<PathBuf>,
}

impl Updates {
    fn new(facts: SharedFacts, proxy: EventLoopProxy<UserEvent>) -> Self {
        let state_path = crate::update::default_state_path().ok();
        let state = state_path
            .as_deref()
            .map(UpdateState::load_at)
            .unwrap_or_default();
        Self {
            client: update_flow::http_client().ok(),
            facts,
            pending: None,
            proxy,
            state,
            state_path,
        }
    }

    fn mode(&self) -> UpdateMode {
        self.facts
            .lock()
            .ok()
            .and_then(|f| UpdateMode::parse(&f.updates))
            .unwrap_or_default()
    }

    fn due(&self) -> bool {
        if self.mode() == UpdateMode::Off {
            return false;
        }
        let elapsed = now_ms().saturating_sub(self.state.last_check_ms);
        elapsed >= CHECK_INTERVAL.as_millis() as i64
    }

    fn persist(&mut self) {
        if let Some(path) = self.state_path.as_deref() {
            let _ = self.state.save_at(path);
        }
    }

    fn announce(&self) {
        let _ = self.proxy.send_event(UserEvent::Facts);
    }

    async fn check(&mut self, manual: bool) {
        if !manual && self.mode() == UpdateMode::Off {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let now = now_ms();
        self.state.last_check_ms = now;
        self.persist();
        if manual {
            // Feedback before the network answers: the button reads "Checking…".
            let pending = self.pending.as_ref();
            with_facts(&self.facts, |f| {
                f.update = Some(UpdateFact {
                    error: String::new(),
                    state: "checking".into(),
                    url: pending.map(|r| r.html_url.clone()).unwrap_or_default(),
                    version: pending.map(|r| r.version.clone()).unwrap_or_default(),
                });
            });
            self.announce();
        }
        let outcome = update_flow::check(&client, env!("CARGO_PKG_VERSION")).await;
        match outcome {
            Ok(Some(release)) => {
                let snoozed =
                    !manual && self.state.snoozed_version.as_deref() == Some(&release.version);
                let fact = UpdateFact {
                    error: String::new(),
                    state: "available".into(),
                    url: release.html_url.clone(),
                    version: release.version.clone(),
                };
                self.pending = Some(release);
                with_facts(&self.facts, |f| {
                    f.update_checked_at = now;
                    f.update = if snoozed { None } else { Some(fact) };
                });
                self.announce();
                if self.mode() == UpdateMode::Auto && !cfg!(debug_assertions) {
                    self.install().await;
                }
            }
            Ok(None) => {
                self.pending = None;
                with_facts(&self.facts, |f| {
                    f.update_checked_at = now;
                    f.update = None;
                });
                self.announce();
            }
            Err(error) => {
                // A background check that fails (offline, rate limited) stays
                // quiet; a manual one owes the user an answer.
                with_facts(&self.facts, |f| {
                    f.update_checked_at = now;
                    if manual {
                        f.update = Some(UpdateFact {
                            error,
                            state: "failed".into(),
                            url: String::new(),
                            version: String::new(),
                        });
                    }
                });
                self.announce();
            }
        }
    }

    /// Install the pending release. The caller decides what "nothing
    /// pending" means (a failed check's Try Again re-checks instead).
    async fn install(&mut self) {
        let Some(release) = self.pending.clone() else {
            return;
        };
        let Some(client) = self.client.clone() else {
            return;
        };
        let set_state = |facts: &SharedFacts, state: &str, error: String| {
            with_facts(facts, |f| {
                f.update = Some(UpdateFact {
                    error,
                    state: state.into(),
                    url: release.html_url.clone(),
                    version: release.version.clone(),
                });
            });
        };
        set_state(&self.facts, "downloading", String::new());
        self.announce();
        match update_flow::install(&client, &release).await {
            Ok(exe) => {
                set_state(&self.facts, "installing", String::new());
                self.announce();
                let _ = self.proxy.send_event(UserEvent::Restart(exe));
            }
            Err(error) => {
                set_state(&self.facts, "failed", error);
                self.announce();
            }
        }
    }

    fn snooze(&mut self) {
        if let Some(release) = self.pending.as_ref() {
            self.state.snoozed_version = Some(release.version.clone());
            self.persist();
        }
        with_facts(&self.facts, |f| f.update = None);
        self.announce();
    }

    async fn set_mode(&mut self, mode: UpdateMode) {
        with_facts(&self.facts, |f| f.updates = mode.as_str().into());
        self.announce();
        match mode {
            UpdateMode::Auto if self.pending.is_some() && !cfg!(debug_assertions) => {
                self.install().await;
            }
            UpdateMode::Auto | UpdateMode::Notify if self.due() => self.check(false).await,
            _ => {}
        }
    }
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
    let severity = worst_severity(&payload);
    if let Ok(icon) = icon_from_severity(severity) {
        let _ = state.tray.set_icon(Some(icon));
    }
    // No hover tip: the popover is the readout. The one exception names the
    // missing WebView2 runtime, because without it there is no popover.
    if state.webview.is_none() {
        let _ = state.tray.set_tooltip(Some(WEBVIEW2_MISSING));
    }
    state.payload = payload;
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
    let severity = worst_severity(&state.payload);
    if let Ok(icon) = icon_from_severity(severity) {
        let _ = state.tray.set_icon(Some(icon));
    }
    if state.js_ready {
        push_to_webview(state);
    }
}

/// Shortcut press: toggle, and focus at once (no tray mouse-up to absorb).
fn toggle_popover_from_keyboard(state: &mut TrayState) {
    if state.popover_open {
        hide_popover(state);
    } else {
        let rect = state.last_tray_rect;
        show_popover(state, rect);
        guard_blur(state);
        state.window.set_focus();
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
            if state.popover_open {
                hide_popover(state);
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

/// Logical height of the monitor the popover sits on (primary as fallback).
fn work_area_height(window: &Window) -> f64 {
    window
        .current_monitor()
        .or_else(|| window.primary_monitor())
        .map(|monitor| {
            let scale = monitor.scale_factor();
            let scale = if scale.is_finite() && scale > 0.0 {
                scale
            } else {
                1.0
            };
            f64::from(monitor.size().height) / scale
        })
        .unwrap_or(FALLBACK_WORK_AREA_HEIGHT)
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
    position_window(&state.window, tray_rect);
    guard_blur(state);
    state.window.set_visible(true);
    // Do not focus yet: the tray mouse-up would land on the ⋮ in the footer
    // (the popover sits directly above the NotifyIcon) and open the menu.
    state.popover_open = true;
    if let Some(webview) = state.webview.as_ref() {
        let _ = webview.evaluate_script(&format!(
            "window.__AIUB_LOCKCLICKS__ && window.__AIUB_LOCKCLICKS__({CLICK_LOCK_MS})"
        ));
        let _ = webview.evaluate_script("window.__AIUB_VISIBLE__ && window.__AIUB_VISIBLE__(true)");
    }
    if state.js_ready {
        push_to_webview(state);
    }
    let proxy = state.proxy.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(CLICK_LOCK_MS));
        let _ = proxy.send_event(UserEvent::FocusPopover);
    });
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
        let buttons = (GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000 != 0)
            || (GetAsyncKeyState(VK_RBUTTON as i32) as u16 & 0x8000 != 0);
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

enum BlurVerdict {
    Hide,
    Keep(&'static str),
}

/// Windows shell surfaces that activate themselves now and then (a badge
/// refresh, an overflow relayout) without the user touching them.
const SHELL_CLASSES: [&str; 4] = [
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "TrayNotifyWnd",
    "NotifyIconOverflowWindow",
];

/// The popover is transient: it closes when the user goes somewhere else. A
/// blur says only that *something* took the foreground, so look at what did.
/// Our own windows (the WebView2 child, a context menu) and a taskbar that
/// activated itself with no mouse button down are not "somewhere else".
fn blur_verdict() -> BlurVerdict {
    // SAFETY: plain user32 queries with valid out-pointers.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return BlurVerdict::Hide;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == GetCurrentProcessId() {
            return BlurVerdict::Keep("focus stayed in this process");
        }
        let class = window_class(hwnd);
        let button_down = (GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000 != 0)
            || (GetAsyncKeyState(VK_RBUTTON as i32) as u16 & 0x8000 != 0);
        if !button_down && SHELL_CLASSES.iter().any(|c| *c == class) {
            return BlurVerdict::Keep("taskbar activated itself");
        }
        BlurVerdict::Hide
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
    // Closing resets navigation (OpenUsage: scroll to top, Customize / Settings
    // close) so the next open lands on the dashboard.
    if let Some(webview) = state.webview.as_ref() {
        let _ =
            webview.evaluate_script("window.__AIUB_VISIBLE__ && window.__AIUB_VISIBLE__(false)");
    }
}

fn position_window(window: &Window, tray_rect: Option<Rect>) {
    let size = window.outer_size();
    let (x, y, hint_x, hint_y) = if let Some(rect) = tray_rect {
        let tray_x = rect.position.x.round() as i32;
        let tray_y = rect.position.y.round() as i32;
        let tray_w = rect.size.width as i32;
        let x = tray_x + tray_w / 2 - size.width as i32 / 2;
        let y = tray_y - size.height as i32 - 8;
        (
            x,
            y,
            rect.position.x + f64::from(rect.size.width) / 2.0,
            rect.position.y + f64::from(rect.size.height) / 2.0,
        )
    } else if let Some(monitor) = window.primary_monitor() {
        let screen = monitor.size();
        let origin = monitor.position();
        (
            origin.x + screen.width as i32 - size.width as i32 - 16,
            origin.y + screen.height as i32 - size.height as i32 - 72,
            f64::from(origin.x) + f64::from(screen.width) / 2.0,
            f64::from(origin.y) + f64::from(screen.height) / 2.0,
        )
    } else {
        (80, 80, 80.0, 80.0)
    };
    let (x, y) = clamp_to_monitor(
        window,
        x,
        y,
        size.width as i32,
        size.height as i32,
        hint_x,
        hint_y,
    );
    window.set_outer_position(PhysicalPosition::new(x, y));
}

fn clamp_to_monitor(
    window: &Window,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    hint_x: f64,
    hint_y: f64,
) -> (i32, i32) {
    let Some(monitor) = window
        .monitor_from_point(hint_x, hint_y)
        .or_else(|| window.primary_monitor())
    else {
        return (x.max(8), y.max(8));
    };
    let origin = monitor.position();
    let screen = monitor.size();
    let min_x = origin.x + 8;
    let min_y = origin.y + 8;
    let max_x = origin.x + screen.width as i32 - width - 8;
    let max_y = origin.y + screen.height as i32 - height - 8;
    (
        x.clamp(min_x, max_x.max(min_x)),
        y.clamp(min_y, max_y.max(min_y)),
    )
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

fn build_tray(payload: &Value) -> Result<TrayIcon, String> {
    let icon = icon_from_severity(worst_severity(payload)).map_err(|error| error.to_string())?;
    TrayIconBuilder::new()
        .with_icon(icon)
        .with_menu_on_left_click(false)
        .build()
        .map_err(|error| error.to_string())
}

/// The raster matching the shell's small-icon size for the current DPI
/// (`SM_CXSMICON`: 16 px at 100 %, 24 px at 150 %), so Windows draws it
/// without a second resample.
fn icon_from_severity(severity: Severity) -> Result<Icon, tray_icon::BadIcon> {
    // SAFETY: GetSystemMetrics has no preconditions and touches no memory.
    let wanted = unsafe { GetSystemMetrics(SM_CXSMICON) };
    let wanted = u32::try_from(wanted).unwrap_or(16).max(16);
    let (rgba, size) = tray_icon_rgba(wanted, severity);
    Icon::from_rgba(rgba, size, size)
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

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
