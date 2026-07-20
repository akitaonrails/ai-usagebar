//! Interactive TUI — one tab per enabled vendor, plus one extra tab per
//! configured Anthropic account (`[[anthropic.accounts]]`, issues #14/#17).
//!
//! Controls:
//!   Tab / l / →   next tab
//!   Shift+Tab / h / ←   prev tab
//!   r   refresh active tab
//!   R   refresh all tabs
//!   q / Esc / Ctrl-C   quit

use std::io;

use ai_usagebar::config::Config;
use ai_usagebar::tui::app::{
    App, REFRESH_INTERVAL, TabId, TabState, refresh_one, tabs_from_config,
};
use ai_usagebar::tui::view::draw;
use ai_usagebar::vendor::HTTP_CLIENT_TIMEOUT;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
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
    let tabs = tabs_from_config(&config);
    if tabs.is_empty() {
        eprintln!(
            "No vendors are enabled in {}. Exiting.",
            ai_usagebar::config::config_path_hint()
        );
        return Ok(());
    }

    let client = Client::builder()
        .timeout(HTTP_CLIENT_TIMEOUT)
        .build()
        .map_err(io::Error::other)?;

    let mut app = App::new_with_primary(tabs, config.ui.primary);

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
    spawn_all(app, client, config, &tx);

    // ONE reader thread for the whole session. Spawning a fresh
    // `spawn_blocking(event::poll)` on every `select!` iteration leaked a
    // blocking task each time another branch won: those tasks kept running and
    // raced each other on `event::read()`, so keypresses could be consumed by
    // an orphan and lost. A dedicated thread also means a slow branch can never
    // delay input.
    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<event::KeyEvent>();
    std::thread::spawn(move || {
        loop {
            // A blocking read is fine here: this thread does nothing else, and
            // the channel send wakes the runtime.
            match event::read() {
                Ok(Event::Key(k)) => {
                    if key_tx.send(k).is_err() {
                        return; // receiver gone: the TUI is shutting down.
                    }
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    });

    let mut tick = tokio::time::interval(REFRESH_INTERVAL);
    tick.tick().await; // consume the immediate tick.

    loop {
        terminal.draw(|f| draw(f, app))?;

        tokio::select! {
            biased;
            // Snapshot results from background tasks.
            Some((generation, tab, state)) = rx.recv() => {
                app.apply_refresh(generation, &tab, state);
            }
            // Periodic auto-refresh of all tabs.
            _ = tick.tick() => {
                spawn_all(app, client, config, &tx);
            }
            // Keyboard events, delivered by the single reader thread.
            maybe_key = key_rx.recv() => {
                let Some(k) = maybe_key else {
                    return Ok(()); // reader thread ended: stdin closed.
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
                    // Settings overlay consumes all keys when open.
                    if let Some(s) = app.settings.as_mut() {
                        use ai_usagebar::tui::settings::{Action as SAction, handle_key as shandle};
                        match shandle(s, k.code, k.modifiers) {
                            SAction::Continue => {}
                            SAction::Close => app.settings = None,
                            SAction::SavedAndClose => {
                                app.settings = None;
                                // Re-load config so the new primary takes effect
                                // on the next render, rebuild the tab set so
                                // account/vendor changes made to config.toml
                                // while the TUI was open appear without a
                                // restart, and queue an immediate refresh of
                                // every tab so newly-set API keys are picked up.
                                // Keep the config we already have if the reload
                                // fails — reverting to defaults would silently
                                // drop the user's real settings mid-session.
                                if let Ok(reloaded) = ai_usagebar::config::Config::load() {
                                    *config = reloaded;
                                }
                                app.set_tabs(tabs_from_config(config));
                                app.select_primary(config.ui.primary);
                                spawn_all(app, client, config, &tx);
                            }
                            SAction::Quit => return Ok(()),
                        }
                        continue;
                    }
                    // Normal key handling (settings closed).
                    if matches!(k.code, KeyCode::Char('s')) {
                        // Prefer the file (it may have changed on disk), but fall
                        // back to the config in memory rather than to defaults.
                        let cfg = ai_usagebar::config::Config::load()
                            .unwrap_or_else(|_| config.clone());
                        app.settings = Some(
                            ai_usagebar::tui::settings::SettingsState::from_config(&cfg),
                        );
                        continue;
                    }
                    if handle_key(app, k.code, k.modifiers) {
                        return Ok(());
                    }
                    // Refresh-on-key handling.
                    if matches!(k.code, KeyCode::Char('r'))
                        && let Some(tab) = app.active_tab_id().cloned()
                    {
                        spawn_one(app, tab, client, config, &tx);
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

fn spawn_all(
    app: &mut App,
    client: &Client,
    config: &Config,
    tx: &mpsc::UnboundedSender<(u64, TabId, TabState)>,
) {
    for tab in app.tabs_meta.clone() {
        spawn_one(app, tab, client, config, tx);
    }
}

fn spawn_one(
    app: &mut App,
    tab: TabId,
    client: &Client,
    config: &Config,
    tx: &mpsc::UnboundedSender<(u64, TabId, TabState)>,
) {
    let tx = tx.clone();
    let client = client.clone();
    let cfg = config.clone();
    let generation = app.tab_generation;
    if let Some(index) = app.tabs_meta.iter().position(|current| current == &tab) {
        app.tabs[index] = TabState::Loading;
    }
    tokio::spawn(async move {
        let state = refresh_one(&client, &cfg, &tab).await;
        let _ = tx.send((generation, tab, state));
    });
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
        KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => {
            app.next_tab();
            false
        }
        KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Left => {
            app.prev_tab();
            false
        }
        _ => false,
    }
}
