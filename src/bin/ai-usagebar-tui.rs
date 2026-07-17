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
use std::time::Duration;

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
    let mut config = Config::load().unwrap_or_default();
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

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = event_loop(&mut terminal, &mut app, &client, &mut config).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    res
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
            // Keyboard events. Poll with a small budget so the select wakes
            // up promptly when nothing else is going on.
            res = tokio::task::spawn_blocking(|| event::poll(Duration::from_millis(150))) => {
                let polled = res.unwrap_or(Ok(false)).unwrap_or(false);
                if polled
                    && let Ok(Event::Key(k)) = event::read()
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
                                *config = ai_usagebar::config::Config::load().unwrap_or_default();
                                app.set_tabs(tabs_from_config(config));
                                app.select_primary(config.ui.primary);
                                spawn_all(app, client, config, &tx);
                            }
                        }
                        continue;
                    }
                    // Normal key handling (settings closed).
                    if matches!(k.code, KeyCode::Char('s')) {
                        let cfg = ai_usagebar::config::Config::load().unwrap_or_default();
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
