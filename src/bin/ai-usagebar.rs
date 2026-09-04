//! Waybar widget binary. The library does all the work — this is just the
//! tokio bootstrap + clap parse.

use ai_usagebar::widget::cli::{AuthProvider, Cli, Command, NousAuthAction};
use ai_usagebar::widget::run::run;
use clap::Parser;

fn main() {
    let (argv, config_path) = split_config_arg();
    if let Some(path) = &config_path {
        if !path.is_file() {
            eprintln!(
                "ai-usagebar: config file not found: {} (create it first, or point --config at an existing file)",
                path.display()
            );
            std::process::exit(2);
        }
        ai_usagebar::config::set_override_path(path);
    }
    let cli = Cli::parse_from(argv);
    if let Some(Command::Account { action }) = &cli.command {
        std::process::exit(ai_usagebar::account::run(action));
    }
    if let Some(Command::Settings { action }) = &cli.command {
        std::process::exit(ai_usagebar::tui::settings::run_cli(action));
    }
    if let Some(Command::Auth { provider }) = &cli.command {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(_) => std::process::exit(1),
        };
        let code = match provider {
            AuthProvider::Nous { action } => rt.block_on(run_nous_auth(action)),
        };
        std::process::exit(code);
    }
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => {
            // Catastrophic — emit the always-valid ⚠ JSON and exit 0.
            println!(
                r#"{{"text":"⚠","tooltip":"failed to create tokio runtime","class":"critical"}}"#
            );
            std::process::exit(0);
        }
    };
    // An administrative report, not the widget: it needs the runtime but must
    // not go through the always-exit-0 Waybar contract — a script piping this
    // deserves a real exit code.
    if let Some(Command::Usage { json }) = &cli.command {
        std::process::exit(rt.block_on(ai_usagebar::report::run(*json)));
    }
    let code = rt.block_on(run(cli));
    std::process::exit(code);
}

/// Extract `--config <PATH>` (or `--config=PATH`) from argv before clap sees
/// it, so the flag is accepted in any position — including alongside a
/// subcommand, which clap's `args_conflicts_with_subcommands` would otherwise
/// reject. Returns the remaining argv (with the program name kept first) and
/// the override, if any. A dangling `--config` without a value is an error.
fn split_config_arg() -> (Vec<std::ffi::OsString>, Option<std::path::PathBuf>) {
    let mut rest = Vec::new();
    let mut config: Option<std::path::PathBuf> = None;
    let mut argv = std::env::args_os();
    if let Some(program) = argv.next() {
        rest.push(program);
    }
    while let Some(arg) = argv.next() {
        let text = arg.to_string_lossy();
        if text == "--config" {
            match argv.next() {
                Some(value) => config = Some(std::path::PathBuf::from(value)),
                None => {
                    eprintln!("ai-usagebar: --config requires a path");
                    std::process::exit(2);
                }
            }
        } else if let Some(value) = text.strip_prefix("--config=") {
            config = Some(std::path::PathBuf::from(value));
        } else {
            rest.push(arg);
        }
    }
    (rest, config)
}

async fn run_nous_auth(action: &NousAuthAction) -> i32 {
    let store = ai_usagebar::nous::credentials::CredentialStore::default();
    match action {
        NousAuthAction::Logout => match store.logout() {
            Ok(()) => {
                println!("Nous Research logout complete");
                0
            }
            Err(error) => {
                eprintln!("Nous Research logout failed: {error}");
                1
            }
        },
        NousAuthAction::Login => {
            let client = match reqwest::Client::builder()
                .timeout(ai_usagebar::vendor::HTTP_CLIENT_TIMEOUT)
                .redirect(ai_usagebar::vendor::same_origin_redirect_policy())
                .build()
            {
                Ok(client) => client,
                Err(_) => {
                    eprintln!("Nous Research login failed: could not initialize HTTP client");
                    return 1;
                }
            };
            let endpoints = ai_usagebar::nous::oauth::Endpoints::default();
            let device =
                match ai_usagebar::nous::oauth::request_device_code(&client, &endpoints).await {
                    Ok(device) => device,
                    Err(error) => {
                        eprintln!("Nous Research login failed: {error}");
                        return 1;
                    }
                };
            println!("Open {}", device.verification_uri);
            println!("Verification code: {}", device.user_code);
            let opener = ai_usagebar::nous::oauth::SystemBrowserOpener;
            if !ai_usagebar::nous::oauth::open_verification_url(
                &device.verification_uri_complete,
                &opener,
            ) {
                eprintln!("Browser opener unavailable; open the URL manually.");
            }
            let token =
                match ai_usagebar::nous::oauth::poll_for_token(&client, &endpoints.token, &device)
                    .await
                {
                    Ok(token) => token,
                    Err(error) => {
                        eprintln!("Nous Research login failed: {error}");
                        return 1;
                    }
                };
            let credential =
                match ai_usagebar::nous::oauth::credential_from_token(token, chrono::Utc::now()) {
                    Ok(credential) => credential,
                    Err(error) => {
                        eprintln!("Nous Research login failed: {error}");
                        return 1;
                    }
                };
            match ai_usagebar::nous::oauth::persist_credential(&store, credential) {
                Ok(()) => {
                    println!("Nous Research login complete");
                    0
                }
                Err(error) => {
                    eprintln!("Nous Research login failed: {error}");
                    1
                }
            }
        }
    }
}
