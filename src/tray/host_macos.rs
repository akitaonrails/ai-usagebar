//! NSStatusItem + WKWebView popover. macOS-only.

use std::borrow::Cow;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use block2::RcBlock;
use fs2::FileExt;
use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, Bool};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSApplication, NSAutoresizingMaskOptions, NSBezierPath, NSButton, NSColor, NSEvent,
    NSGlassEffectView, NSGlassEffectViewStyle, NSImage, NSImageScaling, NSScreen, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowOrderingMode,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};
use objc2_quartz_core::kCACornerCurveContinuous;
use serde_json::{Value, json};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::macos::{
    ActivationPolicy, EventLoopExtMacOS, WindowBuilderExtMacOS, WindowExtMacOS,
};
use tao::window::{Window, WindowBuilder};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use wry::http::{Request, Response, StatusCode, header::CONTENT_TYPE};
use wry::{WebView, WebViewBuilder, WebViewBuilderExtDarwin};

use super::browse;
use super::hotkey::{self, HotkeyBinding};
use super::icon::{Severity, tray_icon_rgba};
use super::menu_bar::{self, UsageWindow};
use super::panel::{
    CLICK_LOCK_MS, CORNER_RADIUS, CocoaRect, FALLBACK_WORK_AREA_HEIGHT, PopoverPlacement,
    WINDOW_HEIGHT, WINDOW_WIDTH, clamp_popover_height, cocoa_popover_frame, menu_bar_bottom_y,
};
use super::payload::{
    AccountSwitchFact, HostFacts, UpdateFact, fact_after_check, host_payload, wrap_report,
};
use super::strip::{
    BARS_PIXEL_SIDE, BARS_POINT_SIDE, Stars, StripStyle, bar_fill, bars_layout, bars_rgba,
    content_from_payload, parse_strip_ipc,
};
use super::update_flow;
use super::{startup, tui_launch};
use crate::config::Config;

const INDEX_HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/popover/index.html"));
const POPOVER_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/popover/popover.css"));
const POPOVER_JS: &str = include_str!(concat!(env!("OUT_DIR"), "/popover/popover.js"));

enum UserEvent {
    Tray(TrayIconEvent),
    Ipc(String),
    Report(Value),
    Entry(Value),
    FocusPopover,
    Hotkey,
    Facts,
}

enum WorkerCmd {
    Refresh,
    RefreshEntry(String),
    Detect,
    CheckUpdate,
    Shutdown,
}

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
}

struct TrayState {
    window: Window,
    webview: Option<WebView>,
    tray: TrayIcon,
    worker: mpsc::Sender<WorkerCmd>,
    proxy: EventLoopProxy<UserEvent>,
    payload: Value,
    js_ready: bool,
    popover_open: bool,
    blur_guard_until: Option<Instant>,
    /// Cocoa (points, y-up) location of the last click / shortcut, used to
    /// keep the popover on that screen instead of tao's primary-display space.
    last_anchor: Option<(f64, f64)>,
    /// Last CSS/logical height from the `resize` IPC; not derived from
    /// `inner_size / scale_factor`, which is wrong after a scale-factor change.
    popover_height: f64,
    theme: Theme,
    facts: SharedFacts,
    hotkey: Option<HotkeyBinding>,
    strip_style: StripStyle,
    stars: Stars,
    strip_order: Vec<String>,
    strip_order_known: bool,
    menu_bar_provider: String,
    menu_bar_show_all: bool,
    menu_bar_hide_value: bool,
    menu_bar_window: UsageWindow,
    menu_bar_chart: bool,
    notifications_enabled: bool,
    notifications_threshold: u8,
}

pub fn run() -> i32 {
    let Some(_lock) = SingleInstance::acquire() else {
        return 0;
    };
    if let Err(error) = run_loop() {
        eprintln!("{error}");
        return 1;
    }
    0
}

fn run_loop() -> Result<(), String> {
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    let proxy = event_loop.create_proxy();

    {
        let proxy = proxy.clone();
        TrayIconEvent::set_event_handler(Some(move |event| {
            let _ = proxy.send_event(UserEvent::Tray(event));
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
        .with_transparent(true)
        .with_has_shadow(true)
        .build(&event_loop)
        .map_err(|error: tao::error::OsError| error.to_string())?;

    let config = Config::load().unwrap_or_default();
    let facts: SharedFacts = Arc::new(Mutex::new(host_facts(&config)));

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
    let (cmd_tx, cmd_rx) = mpsc::channel();
    spawn_worker(proxy.clone(), cmd_rx, facts.clone());
    let _ = cmd_tx.send(WorkerCmd::Refresh);

    let empty = wrap_report("{}", &facts_snapshot(&facts), now_ms(), None);
    let menu_bar_window =
        UsageWindow::parse(config.tray.menu_bar_window.as_deref().unwrap_or("auto"));
    let menu_bar_chart = config.tray.menu_bar_style.as_deref() == Some("bars");
    let menu_bar_show_all = config.tray.menu_bar_show_all();
    let tray = build_tray()?;

    let theme = Theme::Light;
    let webview = build_webview(&window, proxy.clone()).ok();
    round_corners(&window);
    install_glass_background(&window);

    let mut state = TrayState {
        window,
        webview,
        tray,
        worker: cmd_tx,
        proxy: proxy.clone(),
        payload: empty,
        js_ready: false,
        popover_open: false,
        blur_guard_until: None,
        last_anchor: None,
        popover_height: WINDOW_HEIGHT,
        theme,
        facts,
        hotkey: hotkey_binding,
        strip_style: StripStyle::Bars,
        stars: Stars::new(),
        strip_order: Vec::new(),
        strip_order_known: false,
        menu_bar_provider: config
            .tray
            .menu_bar_provider
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| menu_bar::HIGHEST_PROVIDER.into()),
        menu_bar_show_all,
        menu_bar_hide_value: config.tray.menu_bar_hide_value,
        menu_bar_window,
        menu_bar_chart,
        notifications_enabled: config.notifications.enabled,
        notifications_threshold: config.notifications.threshold,
    };
    apply_strip_icon(&mut state);

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UserEvent::Tray(tray_event)) => handle_tray(&mut state, tray_event),
            Event::UserEvent(UserEvent::Ipc(body)) => handle_ipc(&mut state, &body, control_flow),
            Event::UserEvent(UserEvent::Report(payload)) => apply_payload(&mut state, payload),
            Event::UserEvent(UserEvent::Entry(entry)) => apply_entry(&mut state, entry),
            Event::UserEvent(UserEvent::Facts) => apply_facts(&mut state),
            Event::UserEvent(UserEvent::Hotkey) => toggle_popover_from_keyboard(&mut state),
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
                if !blur_guarded(&state) && state.popover_open {
                    hide_popover(&mut state);
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => hide_popover(&mut state),
            Event::LoopDestroyed => {
                let _ = state.worker.send(WorkerCmd::Shutdown);
            }
            _ => {}
        }
    })
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
            run_detection(false);
            loop {
                rt.block_on(push_report(&proxy, &facts));
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
                        Ok(WorkerCmd::CheckUpdate) => {
                            rt.block_on(check_release(&proxy, &facts));
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

fn run_detection(force: bool) {
    if let Ok(state_path) = crate::detect::default_state_path() {
        let _ = crate::detect::run_once(None, &state_path, force);
    }
}

fn host_facts(config: &Config) -> HostFacts {
    let mut facts = HostFacts::new(env!("CARGO_PKG_VERSION"), startup::is_enabled());
    facts.refresh_secs = config.tray.refresh_minutes() * 60;
    facts.accounts = account_facts(config);
    facts
}

/// Which Claude CLI and Codex logins are active, for the switch control on
/// each account's card. Read fresh on every report, so a switch made from the
/// terminal shows up too.
fn account_facts(config: &Config) -> Vec<AccountSwitchFact> {
    let mut out = Vec::new();
    let claude = config.anthropic.all_accounts();
    if config.anthropic.enabled && !claude.is_empty() {
        let active = crate::anthropic::cli_account::home_claude_json()
            .ok()
            .and_then(|home| crate::anthropic::cli_account::resolve_active_label(&home, &claude));
        out.push(AccountSwitchFact {
            vendor: "anthropic".into(),
            active,
            labels: claude.iter().map(|account| account.label.clone()).collect(),
            ..AccountSwitchFact::default()
        });
    }
    let codex = &config.openai.accounts;
    if config.openai.enabled && !codex.is_empty() {
        let active = config
            .openai
            .resolve_auth_path(None)
            .ok()
            .and_then(|default| crate::openai::account::resolve_active_label(&default, codex));
        out.push(AccountSwitchFact {
            vendor: "openai".into(),
            active,
            labels: codex.iter().map(|account| account.label.clone()).collect(),
            ..AccountSwitchFact::default()
        });
    }
    out
}

/// Replace the account facts with a fresh read, keeping any running switch
/// and the last error attached to their vendor.
fn refresh_account_facts(facts: &SharedFacts) {
    let fresh = account_facts(&Config::load().unwrap_or_default());
    with_facts(facts, |f| {
        f.accounts = fresh
            .into_iter()
            .map(|mut fact| {
                if let Some(old) = f.accounts.iter().find(|old| old.vendor == fact.vendor) {
                    fact.target.clone_from(&old.target);
                    fact.switching = old.switching;
                    fact.error.clone_from(&old.error);
                }
                fact
            })
            .collect();
    });
}

/// Run `ai-usagebar account switch` out of process, exactly as a terminal
/// would: the Claude half may quit and reopen the Desktop app, and its errors
/// arrive on stderr, which becomes the card's message. Runs on its own thread,
/// so a slow switch never holds up the refresh worker; the switch is a
/// transaction with its own rollback, so it is left to finish rather than
/// killed on a timer.
fn run_account_switch(facts: &SharedFacts, vendor: &str, label: &str) {
    let error = match resolve_cli() {
        Some(cli) => switch_with(&cli, vendor, label),
        None => "the ai-usagebar CLI was not found next to the tray or in ~/.cargo/bin".into(),
    };
    with_facts(facts, |f| {
        for fact in f.accounts.iter_mut().filter(|fact| fact.vendor == vendor) {
            fact.switching = false;
            fact.error.clone_from(&error);
        }
    });
}

/// The switch itself; returns the error to show, or empty on success.
fn switch_with(cli: &std::path::Path, vendor: &str, label: &str) -> String {
    let mut command = std::process::Command::new(cli);
    command.args(["account", "switch", "--yes"]);
    if vendor == "openai" {
        command.arg("--codex");
    }
    command.arg("--").arg(label);
    match command.stdin(std::process::Stdio::null()).output() {
        Ok(output) if output.status.success() => String::new(),
        Ok(output) => String::from_utf8_lossy(&output.stderr)
            .lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty())
            .map(|line| {
                line.trim_start_matches("ai-usagebar account switch: ")
                    .to_string()
            })
            .unwrap_or_else(|| format!("account switch exited with {}", output.status)),
        Err(error) => format!("could not run ai-usagebar: {error}"),
    }
}

/// The `ai-usagebar` CLI next to this tray binary, then `~/.cargo/bin`.
/// Never a `PATH` lookup: this runs a command that moves logins, so the
/// binary must not be an ambient choice.
fn resolve_cli() -> Option<std::path::PathBuf> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("ai-usagebar");
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    let cargo = crate::cache::home_dir()
        .ok()?
        .join(".cargo")
        .join("bin")
        .join("ai-usagebar");
    cargo.is_file().then_some(cargo)
}

async fn push_report(proxy: &EventLoopProxy<UserEvent>, facts: &SharedFacts) {
    refresh_account_facts(facts);
    let mut snapshot = facts_snapshot(facts);
    snapshot.startup_enabled = startup::is_enabled();
    let now = now_ms();
    let payload = match crate::report::collect_json().await {
        Ok(json) => wrap_report(&json, &snapshot, now, None),
        Err(error) => wrap_report("{}", &snapshot, now, Some(&error)),
    };
    let _ = proxy.send_event(UserEvent::Report(payload));
}

/// Manual GitHub release check. No install on macOS — the About screen opens
/// the release page when a newer tag exists.
async fn check_release(proxy: &EventLoopProxy<UserEvent>, facts: &SharedFacts) {
    with_facts(facts, |f| {
        f.update = Some(UpdateFact {
            error: String::new(),
            state: "checking".into(),
            url: String::new(),
            version: String::new(),
        });
    });
    let _ = proxy.send_event(UserEvent::Facts);
    let outcome = match update_flow::http_client() {
        Ok(client) => update_flow::check(&client, env!("CARGO_PKG_VERSION")).await,
        Err(error) => Err(error),
    };
    let checked_at = now_ms();
    let fact = fact_after_check(outcome);
    with_facts(facts, |f| {
        f.update_checked_at = checked_at;
        f.update = fact;
    });
    let _ = proxy.send_event(UserEvent::Facts);
}

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
    state.payload = payload;
    stamp_facts(state);
    apply_strip_icon(state);
    if state.js_ready {
        push_to_webview(state);
    }
}

fn stamp_facts(state: &mut TrayState) {
    let facts = facts_snapshot(&state.facts);
    let stamped = wrap_report("{}", &facts, 0, None);
    let Some(obj) = state.payload.as_object_mut() else {
        return;
    };
    for key in [
        "shortcut",
        "shortcut_error",
        "refresh_minutes",
        "updates",
        "update",
        "update_checked_at",
        "repository",
        "version",
        "accounts",
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
    apply_strip_icon(state);
    if state.js_ready {
        push_to_webview(state);
    }
}

fn apply_strip_icon(state: &mut TrayState) {
    let content = content_from_payload(&state.payload, &state.stars, &state.strip_order);
    let visible = state
        .strip_order_known
        .then_some(state.strip_order.as_slice());
    let tooltip = menu_bar::tooltip(
        &state.payload,
        &state.menu_bar_provider,
        state.menu_bar_show_all,
        state.menu_bar_window,
        visible,
    );
    let _ = state.tray.set_tooltip(Some(tooltip.as_str()));
    match state.strip_style {
        StripStyle::Bars => {
            let fractions: Vec<f64> = content.bars.iter().map(|m| m.fraction).collect();
            // Keep tray-icon's slot filled so the status item stays allocated,
            // then replace the image with a 1×/2×/3× template that stays sharp
            // on mixed-DPI monitors.
            if let Ok(icon) = bars_icon(&fractions) {
                let _ = state.tray.set_icon(Some(icon));
            }
            state.tray.set_icon_as_template(true);
            if state.menu_bar_chart {
                // tray-icon's macOS set_title(None) leaves the old title in
                // NSStatusBarButton. An empty title actually clears it.
                state.tray.set_title(Some(""));
            } else {
                let title = menu_bar::title(
                    &state.payload,
                    &state.menu_bar_provider,
                    state.menu_bar_show_all,
                    !state.menu_bar_hide_value,
                    state.menu_bar_window,
                    visible,
                );
                state.tray.set_title(Some(title.as_str()));
            }
            if let Some(image) = template_bars_image(&fractions) {
                set_status_button_image(&image);
            }
        }
        StripStyle::Text => {
            if let Ok(icon) = static_icon() {
                let _ = state.tray.set_icon(Some(icon));
            }
            state.tray.set_icon_as_template(true);
            let title = content.title_line();
            state
                .tray
                .set_title((!title.is_empty()).then_some(title.as_str()));
        }
    }
}

fn bars_icon(fractions: &[f64]) -> Result<Icon, tray_icon::BadIcon> {
    let rgba = bars_rgba(fractions, BARS_PIXEL_SIDE);
    Icon::from_rgba(rgba, BARS_PIXEL_SIDE, BARS_PIXEL_SIDE)
}

fn static_icon() -> Result<Icon, tray_icon::BadIcon> {
    let (rgba, size) = tray_icon_rgba(BARS_PIXEL_SIDE, Severity::Low);
    Icon::from_rgba(rgba, size, size)
}

enum ShortcutOutcome {
    Bound(String),
    Cleared,
    Refused { attempted: String, reason: String },
}

fn bind_shortcut(binding: Option<&mut HotkeyBinding>, value: &str) -> ShortcutOutcome {
    let Some(binding) = binding else {
        return ShortcutOutcome::Refused {
            attempted: value.trim().to_string(),
            reason: "Could not set up the global shortcut".into(),
        };
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return match binding.apply(None) {
            Ok(()) => ShortcutOutcome::Cleared,
            Err(reason) => ShortcutOutcome::Refused {
                attempted: String::new(),
                reason,
            },
        };
    }
    match super::hotkey::normalize(trimmed) {
        Ok(normalized) => match binding.apply(Some(&normalized.canonical)) {
            Ok(()) => ShortcutOutcome::Bound(normalized.canonical),
            Err(reason) => ShortcutOutcome::Refused {
                attempted: normalized.canonical,
                reason,
            },
        },
        Err(reason) => ShortcutOutcome::Refused {
            attempted: trimmed.to_string(),
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
    let json = popover_payload(state);
    let script = format!("window.__AIUB_APPLY__ && window.__AIUB_APPLY__({json})");
    let _ = webview.evaluate_script(&script);
}

fn popover_payload(state: &TrayState) -> String {
    let mut payload = state.payload.clone();
    payload["menu_bar_show_all"] = json!(state.menu_bar_show_all);
    payload["menu_bar_hide_value"] = json!(state.menu_bar_hide_value);
    payload["menu_bar_window"] = json!(state.menu_bar_window.as_str());
    payload["menu_bar_provider"] = json!(state.menu_bar_provider);
    payload["menu_bar_chart"] = json!(state.menu_bar_chart);
    payload["notifications_enabled"] = json!(state.notifications_enabled);
    payload["notifications_threshold"] = json!(state.notifications_threshold);
    host_payload(&payload)
}

fn handle_tray(state: &mut TrayState, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        match button {
            MouseButton::Left | MouseButton::Right => {
                if state.popover_open {
                    hide_popover(state);
                } else {
                    state.last_anchor = Some(cocoa_mouse());
                    show_popover(state);
                }
            }
            MouseButton::Middle => next_menu_bar_provider(state),
        }
    }
}

fn persist_menu_bar_value(key: &str, value: toml_edit::Value) {
    if let Some(path) = config_path() {
        let _ = crate::config::set_tray_value(&path, key, Some(value));
    }
}

fn next_menu_bar_provider(state: &mut TrayState) {
    let visible = state
        .strip_order_known
        .then_some(state.strip_order.as_slice());
    if let Some(id) = menu_bar::next_id(
        &state.payload,
        &state.menu_bar_provider,
        state.menu_bar_window,
        visible,
    ) {
        state.menu_bar_provider = id.clone();
        persist_menu_bar_value("menu_bar_provider", id.into());
        // A cycle must visibly change the strip even when Show All was on.
        if state.menu_bar_show_all {
            state.menu_bar_show_all = false;
            persist_menu_bar_value("menu_bar_show_all", false.into());
        }
        apply_strip_icon(state);
    }
}

fn set_menu_bar_window(state: &mut TrayState, window: UsageWindow) {
    state.menu_bar_window = window;
    persist_menu_bar_value("menu_bar_window", window.as_str().into());
    apply_strip_icon(state);
}

/// Start a switch the popover asked for. Only a vendor and label the host
/// itself reported are accepted, and never while one is already running.
fn request_account_switch(state: &mut TrayState, value: &Value) {
    let vendor = value.get("vendor").and_then(Value::as_str).unwrap_or("");
    let label = value.get("label").and_then(Value::as_str).unwrap_or("");
    let allowed = facts_snapshot(&state.facts).accounts.iter().any(|fact| {
        fact.vendor == vendor
            && !fact.switching
            && fact.active.as_deref() != Some(label)
            && fact.labels.iter().any(|known| known == label)
    });
    if !allowed {
        return;
    }
    with_facts(&state.facts, |f| {
        for fact in f.accounts.iter_mut().filter(|fact| fact.vendor == vendor) {
            fact.target = label.to_string();
            fact.switching = true;
            fact.error.clear();
        }
    });
    apply_facts(state);
    let facts = state.facts.clone();
    let proxy = state.proxy.clone();
    let worker = state.worker.clone();
    let failed_vendor = vendor.to_string();
    let (vendor, label) = (vendor.to_string(), label.to_string());
    let spawned = std::thread::Builder::new()
        .name("ai-usagebar-tray-account-switch".into())
        .spawn(move || {
            run_account_switch(&facts, &vendor, &label);
            let _ = proxy.send_event(UserEvent::Facts);
            let _ = worker.send(WorkerCmd::Refresh);
        });
    if spawned.is_err() {
        with_facts(&state.facts, |f| {
            for fact in f
                .accounts
                .iter_mut()
                .filter(|fact| fact.vendor == failed_vendor)
            {
                fact.switching = false;
                fact.error = "could not start the account switch".into();
            }
        });
        apply_facts(state);
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
        "close" => hide_popover(state),
        "quit" => *control_flow = ControlFlow::Exit,
        "toggle-startup" => toggle_startup(state),
        "switch-account" => request_account_switch(state, &value),
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
        "set-refresh" => {
            if let Some(minutes) = value.get("minutes").and_then(Value::as_u64) {
                set_refresh(state, minutes);
            }
        }
        "set-notifications-enabled" => {
            if let Some(enabled) = value.get("value").and_then(Value::as_bool)
                && let Some(path) = config_path()
                && crate::config::set_notification_value(&path, "enabled", enabled.into()).is_ok()
            {
                state.notifications_enabled = enabled;
                push_to_webview(state);
            }
        }
        "set-notifications-threshold" => {
            if let Some(threshold) = value.get("value").and_then(Value::as_u64)
                && (1..=100).contains(&threshold)
                && let Some(path) = config_path()
                && crate::config::set_notification_value(
                    &path,
                    "threshold",
                    (threshold as i64).into(),
                )
                .is_ok()
            {
                state.notifications_threshold = threshold as u8;
                push_to_webview(state);
            }
        }
        "next-menu-bar-provider" => {
            next_menu_bar_provider(state);
            push_to_webview(state);
        }
        "set-menu-bar-provider" => {
            if let Some(id) = value.get("value").and_then(Value::as_str) {
                let eligible = id == menu_bar::HIGHEST_PROVIDER
                    || state
                        .payload
                        .get("entries")
                        .and_then(Value::as_array)
                        .is_some_and(|entries| {
                            entries
                                .iter()
                                .any(|entry| entry.get("id").and_then(Value::as_str) == Some(id))
                        })
                        && (!state.strip_order_known
                            || state.strip_order.iter().any(|shown| shown == id));
                if eligible {
                    state.menu_bar_provider = id.to_owned();
                    persist_menu_bar_value("menu_bar_provider", id.into());
                    if state.menu_bar_show_all {
                        state.menu_bar_show_all = false;
                        persist_menu_bar_value("menu_bar_show_all", false.into());
                    }
                    apply_strip_icon(state);
                    push_to_webview(state);
                }
            }
        }
        "set-menu-bar-show-all" => {
            if let Some(enabled) = value.get("value").and_then(Value::as_bool) {
                state.menu_bar_show_all = enabled;
                persist_menu_bar_value("menu_bar_show_all", enabled.into());
                apply_strip_icon(state);
                push_to_webview(state);
            }
        }
        "set-menu-bar-hide-value" => {
            if let Some(enabled) = value.get("value").and_then(Value::as_bool) {
                state.menu_bar_hide_value = enabled;
                persist_menu_bar_value("menu_bar_hide_value", enabled.into());
                apply_strip_icon(state);
                push_to_webview(state);
            }
        }
        "set-menu-bar-window" => {
            if let Some(window) = value.get("value").and_then(Value::as_str) {
                set_menu_bar_window(state, UsageWindow::parse(window));
                push_to_webview(state);
            }
        }
        "set-menu-bar-chart" => {
            if let Some(enabled) = value.get("value").and_then(Value::as_bool) {
                state.menu_bar_chart = enabled;
                persist_menu_bar_value(
                    "menu_bar_style",
                    (if enabled { "bars" } else { "provider" }).into(),
                );
                apply_strip_icon(state);
                push_to_webview(state);
            }
        }
        "strip" => {
            let (style, stars, order) = parse_strip_ipc(&value);
            state.strip_style = style;
            state.stars = stars;
            state.strip_order = order;
            state.strip_order_known = true;
            apply_strip_icon(state);
        }
        "open-url" => {
            if let Some(url) = value.get("url").and_then(Value::as_str) {
                browse::open(url);
            }
        }
        "check-update" => {
            let _ = state.worker.send(WorkerCmd::CheckUpdate);
        }
        _ => {}
    }
}

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
    let visible_h = anchor_visible_height(state.last_anchor);
    let target = clamp_popover_height(requested, visible_h);
    state.popover_height = target;
    state
        .window
        .set_inner_size(LogicalSize::new(WINDOW_WIDTH, target));
    if state.popover_open {
        position_popover(state);
    }
}

fn apply_theme(state: &mut TrayState, theme: Theme) {
    state.theme = theme;
    let ptr = state.window.ns_window() as *mut NSWindow;
    if let Some(window) = unsafe { ptr.as_ref() } {
        // SAFETY: AppKit exports these immutable appearance names for the
        // lifetime of the process.
        let name = unsafe {
            match theme {
                Theme::Light => NSAppearanceNameAqua,
                Theme::Dark => NSAppearanceNameDarkAqua,
            }
        };
        if let Some(appearance) = NSAppearance::appearanceNamed(name) {
            window.setAppearance(Some(&appearance));
        }
    }
}

fn anchor_visible_height(anchor: Option<(f64, f64)>) -> f64 {
    let (x, y) = anchor.unwrap_or((0.0, 0.0));
    screen_visible_containing(x, y)
        .map(|visible| visible.h)
        .unwrap_or(FALLBACK_WORK_AREA_HEIGHT)
}

fn toggle_startup(state: &mut TrayState) {
    let next = !startup::is_enabled();
    if startup::set_enabled(next).is_ok() {
        if let Some(obj) = state.payload.as_object_mut() {
            obj.insert("startup_enabled".into(), Value::Bool(next));
        }
        if state.js_ready {
            push_to_webview(state);
        }
    }
}

fn show_popover(state: &mut TrayState) {
    if state.last_anchor.is_none() {
        state.last_anchor = Some(cocoa_mouse());
    }
    position_popover(state);
    guard_blur(state);
    state.window.set_visible(true);
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

fn blur_guarded(state: &TrayState) -> bool {
    state
        .blur_guard_until
        .is_some_and(|until| Instant::now() < until)
}

const BLUR_GUARD: Duration = Duration::from_millis(400);

fn guard_blur(state: &mut TrayState) {
    state.blur_guard_until = Some(Instant::now() + BLUR_GUARD);
}

fn hide_popover(state: &mut TrayState) {
    state.window.set_visible(false);
    state.popover_open = false;
    if let Some(webview) = state.webview.as_ref() {
        let _ =
            webview.evaluate_script("window.__AIUB_VISIBLE__ && window.__AIUB_VISIBLE__(false)");
    }
}

fn toggle_popover_from_keyboard(state: &mut TrayState) {
    if state.popover_open {
        hide_popover(state);
    } else {
        show_popover(state);
    }
}

fn position_popover(state: &TrayState) {
    let (icon_x, icon_y) = state.last_anchor.unwrap_or_else(cocoa_mouse);
    let (screen, visible) = screen_pair_containing(icon_x, icon_y).unwrap_or((
        CocoaRect {
            x: 0.0,
            y: 0.0,
            w: 1440.0,
            h: FALLBACK_WORK_AREA_HEIGHT,
        },
        CocoaRect {
            x: 0.0,
            y: 0.0,
            w: 1440.0,
            h: FALLBACK_WORK_AREA_HEIGHT,
        },
    ));
    let below_y = status_bar_bottom_y(screen).unwrap_or_else(|| menu_bar_bottom_y(screen, visible));
    let height = clamp_popover_height(state.popover_height, visible.h);
    let frame = cocoa_popover_frame(PopoverPlacement {
        visible,
        below_y,
        icon_x,
        popover_w: WINDOW_WIDTH,
        popover_h: height,
    });
    apply_cocoa_frame(&state.window, frame);
}

fn build_tray() -> Result<TrayIcon, String> {
    let icon = static_icon().map_err(|error| error.to_string())?;
    // NSStatusItem.setMenu intercepts clicks even when the tray-icon menu-on-
    // click flags are false. Keep the status item menu-free so both mouse
    // buttons reach handle_tray and open the WKWebView panel.
    TrayIconBuilder::new()
        .with_icon(icon)
        .with_icon_as_template(true)
        .with_menu_on_left_click(false)
        .with_menu_on_right_click(false)
        .build()
        .map_err(|error| error.to_string())
}

fn build_webview(window: &Window, proxy: EventLoopProxy<UserEvent>) -> Result<WebView, String> {
    // Stable WKWebsiteDataStore so Customize layout / stars survive restarts
    // (wry has no data_directory on macOS; this is the Darwin stand-in).
    const STORE: [u8; 16] = [
        0xa1, 0x05, 0xa6, 0xeb, 0x74, 0x72, 0x61, 0x79, 0x77, 0x65, 0x62, 0x76, 0x61, 0x69, 0x75,
        0x62,
    ];
    WebViewBuilder::new()
        .with_custom_protocol("aiub".into(), move |_id, request| {
            protocol_response(request)
        })
        // WKWebView registers the custom scheme as `aiub://` (WebView2 uses
        // `http://aiub.localhost/` instead).
        .with_url("aiub://localhost/index.html")
        .with_ipc_handler(move |request| {
            let body = request.body().clone();
            let _ = proxy.send_event(UserEvent::Ipc(body));
        })
        .with_transparent(true)
        .with_background_color((0, 0, 0, 0))
        .with_accept_first_mouse(true)
        .with_data_store_identifier(STORE)
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

fn cocoa_mouse() -> (f64, f64) {
    let point = NSEvent::mouseLocation();
    (point.x, point.y)
}

fn ns_rect_to_cocoa(rect: NSRect) -> CocoaRect {
    CocoaRect {
        x: rect.origin.x,
        y: rect.origin.y,
        w: rect.size.width,
        h: rect.size.height,
    }
}

fn screen_visible_containing(x: f64, y: f64) -> Option<CocoaRect> {
    screen_pair_containing(x, y).map(|(_, visible)| visible)
}

fn screen_pair_containing(x: f64, y: f64) -> Option<(CocoaRect, CocoaRect)> {
    let mtm = MainThreadMarker::new()?;
    let screens = NSScreen::screens(mtm);
    let mut fallback = None;
    for screen in screens.iter() {
        let frame = ns_rect_to_cocoa(screen.frame());
        let visible = ns_rect_to_cocoa(screen.visibleFrame());
        if fallback.is_none() {
            fallback = Some((frame, visible));
        }
        if frame.contains(x, y) {
            return Some((frame, visible));
        }
    }
    fallback
}

/// Bottom edge of the menu bar on `screen`, in Cocoa Y (the popover hangs under this).
fn status_bar_bottom_y(screen: CocoaRect) -> Option<f64> {
    let mtm = MainThreadMarker::new()?;
    let expected = AnyClass::get(c"NSStatusBarWindow")?;
    let app = NSApplication::sharedApplication(mtm);
    let mut best: Option<f64> = None;
    for window in app.windows().iter() {
        if window.class() != expected {
            continue;
        }
        let frame = ns_rect_to_cocoa(window.frame());
        let cx = frame.x + frame.w / 2.0;
        let cy = frame.y + frame.h / 2.0;
        if screen.contains(cx, cy) || (frame.max_y() - screen.max_y()).abs() < 2.0 {
            best = Some(frame.y);
        }
    }
    best
}

/// Match Windows 11 `DWMWCP_ROUND` / OpenUsage's 13pt continuous corners.
/// The React shell is shared; this is the native window clip WKWebView won't
/// get from CSS alone.
fn round_corners(window: &Window) {
    let ptr = window.ns_window() as *mut NSWindow;
    if ptr.is_null() {
        return;
    }
    let ns_window = unsafe { &*ptr };
    ns_window.setOpaque(false);
    ns_window.setBackgroundColor(Some(&NSColor::clearColor()));
    if let Some(view) = ns_window.contentView() {
        round_view(&view);
        for sub in view.subviews().iter() {
            round_view(&sub);
        }
    }
    let ns_view = window.ns_view() as *mut NSView;
    if !ns_view.is_null() {
        round_view(unsafe { &*ns_view });
    }
}

/// Put AppKit's material behind WKWebView. On systems with Liquid Glass, use
/// NSGlassEffectView; older macOS versions use the semantic popover material.
fn install_glass_background(window: &Window) {
    let ptr = window.ns_window() as *mut NSWindow;
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(ns_window) = (unsafe { ptr.as_ref() }) else {
        return;
    };
    let Some(content) = ns_window.contentView() else {
        return;
    };
    let sizing =
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable;

    if AnyClass::get(c"NSGlassEffectView").is_some() {
        let glass = NSGlassEffectView::new(mtm);
        glass.setStyle(NSGlassEffectViewStyle::Regular);
        glass.setFrame(content.bounds());
        glass.setAutoresizingMask(sizing);
        round_view(&glass);
        content.addSubview_positioned_relativeTo(&glass, NSWindowOrderingMode::Below, None);
    } else {
        let material = NSVisualEffectView::new(mtm);
        material.setMaterial(NSVisualEffectMaterial::Popover);
        material.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        material.setState(NSVisualEffectState::Active);
        material.setFrame(content.bounds());
        material.setAutoresizingMask(sizing);
        round_view(&material);
        content.addSubview_positioned_relativeTo(&material, NSWindowOrderingMode::Below, None);
    }
}

fn round_view(view: &NSView) {
    view.setWantsLayer(true);
    let Some(layer) = view.layer() else {
        return;
    };
    layer.setCornerRadius(CORNER_RADIUS);
    layer.setMasksToBounds(true);
    // SAFETY: `kCACornerCurveContinuous` is a process-lifetime CFString.
    unsafe { layer.setCornerCurve(kCACornerCurveContinuous) };
}

fn apply_cocoa_frame(window: &Window, frame: CocoaRect) {
    let ptr = window.ns_window() as *mut NSWindow;
    if ptr.is_null() {
        return;
    }
    let ns_window = unsafe { &*ptr };
    let rect = NSRect {
        origin: NSPoint::new(frame.x, frame.y),
        size: NSSize::new(frame.w, frame.h),
    };
    ns_window.setFrame_display(rect, true);
    sync_screen_properties(ns_window);
}

/// Match the destination display's backing scale and color space so WKWebView
/// text and the status-item glyph aren't painted at the primary's scale.
fn sync_screen_properties(ns_window: &NSWindow) {
    let scale = ns_window
        .screen()
        .map(|screen| {
            if let Some(space) = screen.colorSpace() {
                ns_window.setColorSpace(Some(&space));
            }
            let scale = screen.backingScaleFactor();
            if scale.is_finite() && scale > 0.0 {
                scale
            } else {
                ns_window.backingScaleFactor()
            }
        })
        .unwrap_or_else(|| ns_window.backingScaleFactor());
    if let Some(view) = ns_window.contentView() {
        apply_contents_scale(&view, scale);
        for sub in view.subviews().iter() {
            apply_contents_scale(&sub, scale);
            for nested in sub.subviews().iter() {
                apply_contents_scale(&nested, scale);
            }
        }
    }
}

fn apply_contents_scale(view: &NSView, scale: f64) {
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setContentsScale(scale);
        layer.setNeedsDisplay();
    }
    view.setNeedsDisplay(true);
}

fn template_bars_image(fractions: &[f64]) -> Option<Retained<NSImage>> {
    if fractions.is_empty() {
        return None;
    }
    let fractions = fractions.to_vec();
    let point = f64::from(BARS_POINT_SIDE);
    let block = RcBlock::new(move |dst: NSRect| {
        draw_template_bars(dst, &fractions);
        Bool::from(true)
    });
    let image =
        NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(point, point), true, &block);
    image.setTemplate(true);
    Some(image)
}

fn draw_template_bars(dst: NSRect, fractions: &[f64]) {
    let side = dst.size.width.min(dst.size.height).max(1.0);
    let layout = bars_layout(fractions.len(), side);
    let ox = dst.origin.x;
    let oy = dst.origin.y;
    for (i, fraction) in fractions.iter().copied().take(layout.n).enumerate() {
        let y = oy + layout.y_offset + i as f64 * (layout.track_h + layout.gap) + 1.0;
        fill_round_rect(
            ox + layout.track_x,
            y,
            layout.track_w,
            layout.track_h,
            layout.rx,
            0.16,
        );
        let fill = bar_fill(layout.track_w, fraction);
        if fill.fill_w > 0.0 {
            let trailing = if fill.fill_w >= layout.track_w {
                layout.rx
            } else {
                (layout.rx * 0.35).floor().max(0.0)
            };
            fill_round_rect(
                ox + layout.track_x,
                y,
                fill.fill_w,
                layout.track_h,
                trailing.min(layout.rx),
                1.0,
            );
        }
        if fill.fill_w > 0.0
            && fill.remainder_w > 0.0
            && let Some(divider_x) = fill.divider_x
        {
            fill_round_rect(
                ox + layout.track_x + divider_x,
                y,
                fill.remainder_w,
                layout.track_h,
                layout.rx,
                0.24,
            );
        }
    }
}

fn fill_round_rect(x: f64, y: f64, w: f64, h: f64, radius: f64, alpha: f64) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let radius = radius.max(0.0).min(h * 0.5).min(w * 0.5);
    let rect = NSRect {
        origin: NSPoint::new(x, y),
        size: NSSize::new(w, h),
    };
    NSColor::colorWithWhite_alpha(0.0, alpha).setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius).fill();
}

fn set_status_button_image(image: &NSImage) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(button) = find_status_bar_button(mtm) else {
        return;
    };
    button.setImageScaling(NSImageScaling::ScaleNone);
    button.setImage(Some(image));
}

fn find_status_bar_button(mtm: MainThreadMarker) -> Option<Retained<NSButton>> {
    let app = NSApplication::sharedApplication(mtm);
    let button_class = AnyClass::get(c"NSStatusBarButton")?;
    for window in app.windows().iter() {
        if let Some(root) = window.contentView()
            && let Some(button) = find_classed_button(&root, button_class)
        {
            return Some(button);
        }
    }
    None
}

fn find_classed_button(view: &NSView, class: &AnyClass) -> Option<Retained<NSButton>> {
    if view.class() == class {
        return view.retain().downcast().ok();
    }
    for sub in view.subviews().iter() {
        if let Some(button) = find_classed_button(&sub, class) {
            return Some(button);
        }
    }
    None
}

struct SingleInstance {
    // Held for the process lifetime so the exclusive flock stays taken.
    _lock: File,
}

impl SingleInstance {
    fn acquire() -> Option<Self> {
        let dir = crate::cache::xdg_cache_dir().ok()?.join("ai-usagebar");
        std::fs::create_dir_all(&dir).ok()?;
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(dir.join("tray.lock"))
            .ok()?;
        file.try_lock_exclusive().ok()?;
        let _ = writeln!(&file, "{}", std::process::id());
        Some(Self { _lock: file })
    }
}
