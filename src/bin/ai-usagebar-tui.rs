//! Interactive TUI — one tab per configured, enabled vendor, plus one extra tab
//! per configured Anthropic account (`[[anthropic.accounts]]`, issues #14/#17).
//!
//! Controls:
//!   ↑ / ↓           move through the vendor menu (wraps; mouse clicks work too)
//!   Tab / l / →     next tab (secondary)
//!   Shift+Tab / h / ←   prev tab (secondary)
//!   r   refresh active tab
//!   R   refresh all tabs
//!   c   local Claude Code context sessions (when enabled)
//!   s   settings overlay (mouse clicks select fields)
//!   q / Esc / Ctrl-C   quit

use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use ai_usagebar::config::Config;
use ai_usagebar::tui::app::{
    ANTHROPIC_REFRESH_STAGGER, App, FooterAction, REFRESH_INTERVAL, TabId, TabState, refresh_one,
    refresh_stagger, tabs_with_desktop,
};
use ai_usagebar::tui::view::draw;
use ai_usagebar::vendor::HTTP_CLIENT_TIMEOUT;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::Rect;
use reqwest::Client;
use tokio::sync::mpsc;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("ai-usagebar-tui: {e}");
        std::process::exit(1);
    }
}

async fn run() -> io::Result<()> {
    // Report a broken config instead of silently starting on defaults, and do
    // it before raw mode so the message is actually readable.
    let mut config = Config::load().map_err(|e| {
        io::Error::other(format!(
            "{} could not be loaded: {e}\n\
             Fix the file (or move it aside) and try again.",
            ai_usagebar::config::config_path_hint()
        ))
    })?;
    let tabs = tabs_with_desktop(&config);
    if tabs.is_empty() {
        eprintln!(
            "No vendors are enabled in {}. Exiting.",
            ai_usagebar::config::config_path_hint()
        );
        return Ok(());
    }

    let client = Client::builder()
        .timeout(HTTP_CLIENT_TIMEOUT)
        .redirect(ai_usagebar::vendor::same_origin_redirect_policy())
        .build()
        .map_err(io::Error::other)?;

    let mut app = App::new_with_primary(tabs, config.ui.primary);
    app.context_enabled = config.context.enabled;
    app.overview_vendors = config.ui.overview_vendors.clone();
    app.vendor_box = config.ui.vendor_box();

    // RAII: restoring the terminal must survive an error or a panic in the
    // loop below. Doing it inline left the user in raw mode on the alternate
    // screen with no cursor whenever anything went wrong.
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    event_loop(&mut terminal, &mut app, &client, &mut config).await
}

/// Owns the terminal mode changes and undoes them on drop, in reverse order.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(e) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
            // Do not leave raw mode enabled if only half the setup succeeded.
            let _ = disable_raw_mode();
            return Err(e);
        }
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Best-effort: we are often unwinding, so there is nowhere to report.
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            LeaveAlternateScreen,
            DisableMouseCapture,
            ratatui::crossterm::cursor::Show
        );
        let _ = disable_raw_mode();
    }
}

/// How often to check `config.toml`'s mtime for edits made outside the TUI
/// (a text editor, `ai-usagebar account add`, another tool).
// ponytail: an mtime poll, not a notify(7)/FSEvents watcher — one stat() every
// couple seconds beats pulling in a file-watching crate + its background thread
// for a file that changes a handful of times a session. The macOS menu-bar app
// watches natively (DispatchSource, free via Foundation); the TUI polls.
const CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Cheap identity for the resolved config file. Including the resolved path and
/// length avoids missing a canonical/legacy-path switch or a same-timestamp
/// rewrite on filesystems with coarse mtime resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfigStamp {
    path: PathBuf,
    modified: SystemTime,
    len: u64,
}

/// Stamp of the resolved config file, or `None` when there is no config yet or
/// it can't be stat'd. Re-resolves the path each call, so a config file created
/// after the TUI started is still noticed.
fn config_stamp() -> Option<ConfigStamp> {
    let path = ai_usagebar::config::resolved_path()?;
    let metadata = std::fs::metadata(&path).ok()?;
    Some(ConfigStamp {
        path,
        modified: metadata.modified().ok()?,
        len: metadata.len(),
    })
}

/// Re-read `config.toml` into `config` and rebuild everything the TUI derives
/// from it — the tab set (vendor + `[[anthropic.accounts]]` changes), the
/// overview vendor list, the context toggle — then re-fetch every tab. Returns
/// `false` and touches nothing if the file can't be parsed, so a half-written
/// edit never wipes the session back to defaults; the next poll retries.
///
/// `reselect_primary` snaps back to the configured primary tab — wanted right
/// after an explicit Settings save, but not on a background file-watch reload,
/// where `set_tabs` already clamps the current tab and yanking the user away
/// from where they were browsing would be rude.
fn reload_config(
    app: &mut App,
    config: &mut Config,
    client: &Client,
    tx: &mpsc::UnboundedSender<(u64, TabId, TabState)>,
    reselect_primary: bool,
) -> bool {
    let Ok(reloaded) = Config::load() else {
        return false;
    };
    *config = reloaded;
    app.context_enabled = config.context.enabled;
    app.overview_vendors = config.ui.overview_vendors.clone();
    app.vendor_box = config.ui.vendor_box();
    app.set_tabs(tabs_with_desktop(config));
    if reselect_primary {
        app.select_primary(config.ui.primary);
    }
    spawn_all(app, client, config, tx);
    true
}

async fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    client: &Client,
    config: &mut Config,
) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    // Kick off initial fetches for every vendor in parallel.
    let (tx, mut rx) = mpsc::unbounded_channel::<(u64, TabId, TabState)>();
    let (context_tx, mut context_rx) = mpsc::unbounded_channel::<(
        u64,
        std::result::Result<ai_usagebar::context::ContextScan, String>,
    )>();
    spawn_all(app, client, config, &tx);

    // ONE reader thread for the whole session. Spawning a fresh
    // `spawn_blocking(event::poll)` on every `select!` iteration leaked a
    // blocking task each time another branch won: those tasks kept running and
    // raced each other on `event::read()`, so keypresses could be consumed by
    // an orphan and lost. A dedicated thread also means a slow branch can never
    // delay input.
    //
    // Resize must wake the loop too: discarding `Event::Resize` left the
    // alternate screen at the previous paint size (UI stuck in a corner after
    // maximize, or ghost cells after shrink) until a keypress forced a draw.
    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<InputEvent>();
    std::thread::spawn(move || {
        loop {
            // A blocking read is fine here: this thread does nothing else, and
            // the channel send wakes the runtime.
            match event::read() {
                Ok(Event::Key(k)) => {
                    if input_tx.send(InputEvent::Key(k)).is_err() {
                        return; // receiver gone: the TUI is shutting down.
                    }
                }
                Ok(Event::Mouse(m)) => {
                    if input_tx.send(InputEvent::Mouse(m)).is_err() {
                        return;
                    }
                }
                Ok(Event::Resize(cols, rows)) => {
                    if input_tx.send(InputEvent::Resize { cols, rows }).is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    });

    let mut tick = tokio::time::interval(REFRESH_INTERVAL);
    tick.tick().await; // consume the immediate tick.

    // Watch config.toml for external edits and hot-reload without a restart.
    let mut config_poll = tokio::time::interval(CONFIG_POLL_INTERVAL);
    config_poll.tick().await; // consume the immediate tick.
    let mut last_config_stamp = config_stamp();

    loop {
        terminal.draw(|f| draw(f, app))?;

        tokio::select! {
            biased;
            // Snapshot results from background tasks.
            Some((generation, tab, state)) = rx.recv() => {
                app.apply_refresh(generation, &tab, state);
            }
            // Local transcript scans carry their own generation so a slow
            // pre-`r` result cannot replace a newer scan.
            Some((generation, result)) = context_rx.recv() => {
                if let Some(context) = app.context.as_mut() {
                    context.apply_scan(generation, result);
                }
            }
            // Periodic auto-refresh of all tabs.
            _ = tick.tick() => {
                spawn_all(app, client, config, &tx);
            }
            // Hot-reload config.toml when it changes on disk (external editor,
            // `ai-usagebar account add`, etc.), preserving the current tab.
            _ = config_poll.tick() => {
                let now = config_stamp();
                if now != last_config_stamp
                    && reload_config(app, config, client, &tx, false)
                {
                    // Only consume the stamp after a successful parse. A
                    // half-written file is retried until it becomes valid.
                    last_config_stamp = now;
                }
            }
            // Keyboard + mouse + resize, delivered by the single reader thread.
            maybe_input = input_rx.recv() => {
                let Some(input) = maybe_input else {
                    return Ok(()); // reader thread ended: stdin closed.
                };
                let k = match input {
                    InputEvent::Resize { cols, rows } => {
                        // Prefer resize() over clear(): clear() snapshots the
                        // cursor via DSR (\x1b[6n) and can hang/fail when the
                        // terminal doesn't answer. resize() for Fullscreen
                        // clears the viewport + resets the diff buffer without
                        // that round-trip; the next draw fills the new area.
                        // Ignore the result: a failed resize (e.g. a transient
                        // ioctl error) must not tear down the whole TUI — the
                        // next successful resize or redraw recovers.
                        let _ = terminal.resize(Rect::new(0, 0, cols, rows));
                        continue;
                    }
                    InputEvent::Mouse(m) => {
                        match handle_mouse(app, &m) {
                            Some(MouseAction::Settings(action)) => {
                                if apply_settings_action(
                                    action,
                                    app,
                                    config,
                                    client,
                                    &tx,
                                    &mut last_config_stamp,
                                ) {
                                    return Ok(());
                                }
                            }
                            Some(MouseAction::Footer(FooterAction::Refresh)) => {
                                refresh_active(app, client, config, &tx);
                            }
                            Some(MouseAction::Footer(FooterAction::RefreshAll)) => {
                                spawn_all(app, client, config, &tx);
                            }
                            Some(MouseAction::Footer(FooterAction::Settings)) => {
                                open_settings(app, config);
                            }
                            Some(MouseAction::Footer(FooterAction::Quit)) => return Ok(()),
                            None => {}
                        }
                        continue;
                    }
                    InputEvent::Key(k) => k,
                };
                {
                    // On Windows Terminal (and terminals advertising the
                    // Kitty keyboard protocol) crossterm reports key Repeat
                    // (auto-repeat while held) and Release events in addition
                    // to Press. Acting on anything but Press makes one tap
                    // move several tabs and holding a key fly through them.
                    // Treat each *press* as exactly one action; ignore
                    // Repeat and Release entirely.
                    if k.kind != KeyEventKind::Press {
                        continue;
                    }
                    // Context overlay consumes all keys while open.
                    if app.context.is_some() {
                        use ai_usagebar::tui::context::{Action as CAction, handle_key as chandle};
                        let action = {
                            let context = app.context.as_mut().expect("checked above");
                            chandle(context, k.code, k.modifiers)
                        };
                        match action {
                            CAction::Continue => {}
                            CAction::Close => app.context = None,
                            CAction::Refresh => {
                                spawn_context_scan(app, config, &context_tx);
                            }
                            CAction::Quit => return Ok(()),
                        }
                        continue;
                    }
                    // Settings overlay consumes all keys when open.
                    if let Some(s) = app.settings.as_mut() {
                        use ai_usagebar::tui::settings::handle_key as shandle;
                        let action = shandle(s, k.code, k.modifiers);
                        if apply_settings_action(
                            action,
                            app,
                            config,
                            client,
                            &tx,
                            &mut last_config_stamp,
                        ) {
                            return Ok(());
                        }
                        continue;
                    }
                    // Normal key handling (settings closed).
                    if matches!(k.code, KeyCode::Char('s')) {
                        open_settings(app, config);
                        continue;
                    }
                    if matches!(k.code, KeyCode::Char('c'))
                        && !k.modifiers.intersects(
                            KeyModifiers::CONTROL
                                | KeyModifiers::ALT
                                | KeyModifiers::SUPER
                                | KeyModifiers::HYPER
                                | KeyModifiers::META,
                        )
                        && app.context_enabled
                    {
                        app.context = Some(ai_usagebar::tui::context::ContextState::new(
                            config.context.layout,
                        ));
                        spawn_context_scan(app, config, &context_tx);
                        continue;
                    }
                    if handle_key(app, k.code, k.modifiers) {
                        return Ok(());
                    }
                    // Refresh-on-key handling.
                    if matches!(k.code, KeyCode::Char('r')) {
                        refresh_active(app, client, config, &tx);
                    }
                    if matches!(k.code, KeyCode::Char('R')) {
                        spawn_all(app, client, config, &tx);
                    }
                }
            }
        }

        if app.quit {
            return Ok(());
        }
    }
}

/// Crossterm events the dedicated reader thread forwards into the async loop.
enum InputEvent {
    Key(event::KeyEvent),
    Mouse(event::MouseEvent),
    Resize { cols: u16, rows: u16 },
}

/// An effect requested by a click after it has been hit-tested against the
/// most recent draw.
enum MouseAction {
    Settings(ai_usagebar::tui::settings::Action),
    Footer(FooterAction),
}

fn spawn_context_scan(
    app: &mut App,
    config: &Config,
    tx: &mpsc::UnboundedSender<(
        u64,
        std::result::Result<ai_usagebar::context::ContextScan, String>,
    )>,
) {
    let Some(context) = app.context.as_mut() else {
        return;
    };
    app.context_generation = app.context_generation.wrapping_add(1);
    let generation = app.context_generation;
    context.begin_refresh(generation);
    let context_config = config.context.clone();
    let tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        let result = (|| {
            let path = match context_config.projects_path.as_deref() {
                Some(path) => path.to_path_buf(),
                None => ai_usagebar::context::default_projects_path()?,
            };
            ai_usagebar::context::scan_dir(&path, &context_config)
        })()
        .map_err(|error| error.to_string());
        let _ = tx.send((generation, result));
    });
}

fn spawn_all(
    app: &mut App,
    client: &Client,
    config: &Config,
    tx: &mpsc::UnboundedSender<(u64, TabId, TabState)>,
) {
    let tabs = app.tabs_meta.clone();
    // Space out the Anthropic tabs so several accounts don't burst the shared
    // usage/token endpoint and trip its rate limit (429).
    let delays = refresh_stagger(&tabs, ANTHROPIC_REFRESH_STAGGER);
    for (tab, delay) in tabs.into_iter().zip(delays) {
        spawn_one(app, tab, client, config, tx, delay);
    }
}

fn spawn_one(
    app: &mut App,
    tab: TabId,
    client: &Client,
    config: &Config,
    tx: &mpsc::UnboundedSender<(u64, TabId, TabState)>,
    delay: Duration,
) {
    if !app.begin_refresh(&tab) {
        return;
    }
    let tx = tx.clone();
    let client = client.clone();
    let cfg = config.clone();
    let generation = app.tab_generation;
    tokio::spawn(async move {
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let state = refresh_one(&client, &cfg, &tab).await;
        let _ = tx.send((generation, tab, state));
    });
}

fn refresh_active(
    app: &mut App,
    client: &Client,
    config: &Config,
    tx: &mpsc::UnboundedSender<(u64, TabId, TabState)>,
) {
    if app.overview {
        // No single active tab on the Overview — refresh all.
        spawn_all(app, client, config, tx);
    } else if let Some(tab) = app.active_tab_id().cloned() {
        // A manual single-tab refresh isn't a burst — no stagger.
        spawn_one(app, tab, client, config, tx, Duration::ZERO);
    }
}

fn open_settings(app: &mut App, config: &Config) {
    // Prefer the file (it may have changed on disk), but fall back to the
    // config in memory rather than to defaults.
    let cfg = ai_usagebar::config::Config::load().unwrap_or_else(|_| config.clone());
    app.settings = Some(ai_usagebar::tui::settings::SettingsState::from_config(&cfg));
}

fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) -> bool {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.quit = true;
            true
        }
        KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => {
            app.quit = true;
            true
        }
        // Up/Down are the primary vendor-menu keys (vertical navigation);
        // Tab/l/←/→ remain as secondary aliases.
        KeyCode::Down | KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => {
            app.next_tab();
            false
        }
        KeyCode::Up | KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Left => {
            app.prev_tab();
            false
        }
        _ => false,
    }
}

/// Hit-test a mouse click against the rects the last draw recorded. Returns an
/// action only when the event needs work from the event loop; focus and tab
/// selection mutate `app` directly.
fn handle_mouse(app: &mut App, m: &event::MouseEvent) -> Option<MouseAction> {
    use ai_usagebar::tui::settings::{Focus as SFocus, SettingsRow};
    use ratatui::layout::Position;

    if m.kind != MouseEventKind::Down(MouseButton::Left) {
        return None;
    }
    let pos = Position::new(m.column, m.row);

    if let Some(s) = app.settings.as_mut() {
        let hit = app.hit.borrow();
        let (row, _) = hit
            .settings_rows
            .iter()
            .find(|(_, rect)| rect.contains(pos))?;
        let row = *row;
        drop(hit);
        match row {
            SettingsRow::MoreHeader => {
                s.toggle_more();
                None
            }
            SettingsRow::Focus(SFocus::Save) => {
                // A click on Save is a save: move focus there and fire Enter
                // through the normal key handler so save logic stays in one
                // place and the returned action flows back to the loop.
                s.focus = SFocus::Save;
                Some(MouseAction::Settings(
                    ai_usagebar::tui::settings::handle_key(s, KeyCode::Enter, KeyModifiers::NONE),
                ))
            }
            SettingsRow::Focus(focus) => {
                s.focus = focus;
                None
            }
        }
    } else {
        let hit = app.hit.borrow();
        let nav_target = hit
            .nav_entries
            .iter()
            .find(|(_, rect)| rect.contains(pos))
            .map(|(target, _)| *target);
        let footer_action = hit
            .footer_actions
            .iter()
            .find(|(_, rect)| rect.contains(pos))
            .map(|(action, _)| *action);
        drop(hit);
        if let Some(target) = nav_target {
            app.nav_from_target(target);
            None
        } else {
            footer_action.map(MouseAction::Footer)
        }
    }
}

/// Apply a settings overlay action. Returns `true` when the loop should quit.
fn apply_settings_action(
    action: ai_usagebar::tui::settings::Action,
    app: &mut App,
    config: &mut Config,
    client: &Client,
    tx: &mpsc::UnboundedSender<(u64, TabId, TabState)>,
    last_config_stamp: &mut Option<ConfigStamp>,
) -> bool {
    use ai_usagebar::tui::settings::Action as SAction;
    match action {
        SAction::Continue => false,
        SAction::Close => {
            app.settings = None;
            false
        }
        SAction::SavedAndClose => {
            app.settings = None;
            // Reload config and rebuild the tab set so a just-saved primary /
            // account / vendor / API-key change takes effect without a
            // restart, snapping to the configured primary since the user just
            // asked for it. A broken reload keeps the current config rather
            // than reverting to defaults.
            if reload_config(app, config, client, tx, true) {
                // The save just rewrote config.toml; adopt its new stamp so
                // the poll doesn't reload again.
                *last_config_stamp = config_stamp();
            }
            false
        }
        SAction::Quit => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_usagebar::theme::Theme;

    fn app_with_two() -> App {
        App::with_theme(
            vec![
                TabId::vendor(ai_usagebar::vendor::VendorId::Anthropic),
                TabId::vendor(ai_usagebar::vendor::VendorId::Openai),
            ],
            Theme::default(),
        )
    }

    #[test]
    fn up_down_navigate_the_vendor_menu() {
        let mut app = app_with_two();
        app.overview = true;
        assert!(!handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE));
        assert!(!app.overview);
        assert_eq!(app.active, 0);
        assert!(!handle_key(&mut app, KeyCode::Up, KeyModifiers::NONE));
        assert!(app.overview);
        // Secondary aliases still work.
        assert!(!handle_key(&mut app, KeyCode::Tab, KeyModifiers::NONE));
        assert!(!app.overview);
        assert_eq!(app.active, 0);
        assert!(!handle_key(&mut app, KeyCode::BackTab, KeyModifiers::NONE));
        assert!(app.overview);
    }

    #[test]
    fn quit_keys_still_quit() {
        let mut app = app_with_two();
        assert!(handle_key(&mut app, KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.quit);
        let mut app = app_with_two();
        assert!(handle_key(
            &mut app,
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        ));
        assert!(app.quit);
    }

    #[test]
    fn clicking_footer_actions_returns_their_matching_action() {
        use ai_usagebar::tui::app::FooterAction;
        use ratatui::layout::Rect;

        let mut app = app_with_two();
        app.hit.borrow_mut().footer_actions = vec![
            (FooterAction::Refresh, Rect::new(0, 0, 8, 1)),
            (FooterAction::RefreshAll, Rect::new(8, 0, 12, 1)),
            (FooterAction::Settings, Rect::new(20, 0, 10, 1)),
            (FooterAction::Quit, Rect::new(30, 0, 10, 1)),
        ];

        for (column, expected) in [
            (0, FooterAction::Refresh),
            (8, FooterAction::RefreshAll),
            (20, FooterAction::Settings),
            (30, FooterAction::Quit),
        ] {
            let click = event::MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row: 0,
                modifiers: KeyModifiers::NONE,
            };
            assert!(matches!(
                handle_mouse(&mut app, &click),
                Some(MouseAction::Footer(action)) if action == expected
            ));
        }
    }
}
