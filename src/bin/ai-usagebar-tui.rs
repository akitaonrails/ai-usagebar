//! Interactive TUI — one tab per enabled vendor.
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
use ai_usagebar::tui::app::{App, REFRESH_INTERVAL, TabState, refresh_one};
use ai_usagebar::tui::view::draw;
use chrono::Utc;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
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
    let config = Config::load().unwrap_or_default();
    let vendors = config.enabled_vendors();
    if vendors.is_empty() {
        eprintln!("No vendors are enabled in ~/.config/ai-usagebar/config.toml. Exiting.");
        return Ok(());
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(io::Error::other)?;

    let mut app = App::new(vendors);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = event_loop(&mut terminal, &mut app, &client, &config).await;

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
    config: &Config,
) -> io::Result<()> {
    // Kick off initial fetches for every vendor in parallel.
    let (tx, mut rx) = mpsc::unbounded_channel::<(usize, TabState)>();
    for (i, v) in app.vendors.clone().into_iter().enumerate() {
        let tx = tx.clone();
        let client = client.clone();
        let cfg = config.clone();
        let theme = app.theme.clone();
        tokio::spawn(async move {
            let state = refresh_one(&client, &cfg, &theme, v).await;
            let _ = tx.send((i, state));
        });
    }

    let mut tick = tokio::time::interval(REFRESH_INTERVAL);
    tick.tick().await; // consume the immediate tick.

    loop {
        terminal.draw(|f| draw(f, app))?;

        tokio::select! {
            biased;
            // Snapshot results from background tasks.
            Some((idx, state)) = rx.recv() => {
                if let Some(slot) = app.tabs.get_mut(idx) {
                    *slot = state;
                    app.last_refresh = Utc::now();
                }
            }
            // Periodic auto-refresh of all tabs.
            _ = tick.tick() => {
                spawn_all(app, client, config, &tx);
            }
            // Keyboard events. Poll with a small budget so the select wakes
            // up promptly when nothing else is going on.
            res = tokio::task::spawn_blocking(|| event::poll(Duration::from_millis(150))) => {
                let polled = res.unwrap_or(Ok(false)).unwrap_or(false);
                if polled {
                    if let Ok(Event::Key(k)) = event::read() {
                        if handle_key(app, k.code, k.modifiers) {
                            return Ok(());
                        }
                        // Refresh-on-key handling.
                        if matches!(k.code, KeyCode::Char('r')) {
                            if let Some(v) = app.active_vendor() {
                                let idx = app.active;
                                let tx = tx.clone();
                                let client = client.clone();
                                let cfg = config.clone();
                                let theme = app.theme.clone();
                                app.tabs[idx] = TabState::Loading;
                                tokio::spawn(async move {
                                    let state = refresh_one(&client, &cfg, &theme, v).await;
                                    let _ = tx.send((idx, state));
                                });
                            }
                        }
                        if matches!(k.code, KeyCode::Char('R')) {
                            spawn_all(app, client, config, &tx);
                        }
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
    tx: &mpsc::UnboundedSender<(usize, TabState)>,
) {
    for (i, v) in app.vendors.clone().into_iter().enumerate() {
        let tx = tx.clone();
        let client = client.clone();
        let cfg = config.clone();
        let theme = app.theme.clone();
        app.tabs[i] = TabState::Loading;
        tokio::spawn(async move {
            let state = refresh_one(&client, &cfg, &theme, v).await;
            let _ = tx.send((i, state));
        });
    }
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
