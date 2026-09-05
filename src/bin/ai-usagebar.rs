//! Waybar widget binary. The library does all the work — this is just the
//! tokio bootstrap + clap parse.

use ai_usagebar::widget::cli::{AuthProvider, Cli, Command, NousAuthAction};
use ai_usagebar::widget::run::run;
use clap::Parser;

fn main() {
    let (argv, config_path) = match split_config_arg(std::env::args_os()) {
        Ok(split) => split,
        Err(message) => {
            eprintln!("ai-usagebar: {message}");
            std::process::exit(2);
        }
    };
    // Parse first so `--version`/`--help` self-report regardless of the
    // `--config` file's state; only then validate the override path, which
    // must be set before any config is read.
    let cli = Cli::parse_from(argv);
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
/// the override, if any. A bare `--` ends the scan: it and everything after it
/// are positional (clap convention) and pass through untouched. A dangling
/// `--config` and a repeated `--config` are errors, matching `ai-usagebar-tui`.
fn split_config_arg<I>(
    argv: I,
) -> Result<(Vec<std::ffi::OsString>, Option<std::path::PathBuf>), String>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    let mut argv = argv.into_iter();
    let mut rest = Vec::new();
    let mut config: Option<std::path::PathBuf> = None;
    if let Some(program) = argv.next() {
        rest.push(program);
    }
    while let Some(arg) = argv.next() {
        if arg == "--" {
            rest.push(arg);
            rest.extend(argv);
            break;
        }
        // Compare at the OsStr level so a path the platform can store but
        // UTF-8 cannot represent is never mangled by a lossy conversion.
        let value = if arg == "--config" {
            match argv.next() {
                Some(value) => Some(std::path::PathBuf::from(value)),
                None => return Err("--config requires a path".into()),
            }
        } else {
            ai_usagebar::config::config_flag_value(&arg)
        };
        if let Some(value) = value {
            if config.is_some() {
                return Err("--config given more than once".into());
            }
            config = Some(value);
        } else {
            rest.push(arg);
        }
    }
    Ok((rest, config))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<std::ffi::OsString> {
        items.iter().map(std::ffi::OsString::from).collect()
    }

    #[test]
    fn flag_is_extracted_before_the_subcommand() {
        let (rest, config) = split_config_arg(argv(&[
            "ai-usagebar",
            "--config",
            "a.toml",
            "account",
            "add",
            "work",
        ]))
        .unwrap();
        assert_eq!(config.as_deref(), Some(std::path::Path::new("a.toml")));
        assert_eq!(rest, argv(&["ai-usagebar", "account", "add", "work"]));
    }

    #[test]
    fn equals_form_is_extracted_after_the_subcommand() {
        let (rest, config) =
            split_config_arg(argv(&["ai-usagebar", "usage", "--config=a.toml", "--json"])).unwrap();
        assert_eq!(config.as_deref(), Some(std::path::Path::new("a.toml")));
        assert_eq!(rest, argv(&["ai-usagebar", "usage", "--json"]));
    }

    #[test]
    fn tokens_after_double_dash_pass_through_untouched() {
        let (rest, config) = split_config_arg(argv(&[
            "ai-usagebar",
            "account",
            "add",
            "--",
            "--config",
            "work",
        ]))
        .unwrap();
        assert!(
            config.is_none(),
            "`--config` behind `--` is a positional, not the flag"
        );
        assert_eq!(
            rest,
            argv(&["ai-usagebar", "account", "add", "--", "--config", "work"])
        );
    }

    #[test]
    fn duplicate_flag_is_rejected_in_every_form() {
        for repeated in [
            &["ai-usagebar", "--config", "a.toml", "--config", "b.toml"][..],
            &["ai-usagebar", "--config=a.toml", "--config=b.toml"][..],
            &["ai-usagebar", "--config", "a.toml", "--config=b.toml"][..],
        ] {
            let err = split_config_arg(argv(repeated)).unwrap_err();
            assert_eq!(err, "--config given more than once");
        }
    }

    #[test]
    fn dangling_flag_is_rejected() {
        let err = split_config_arg(argv(&["ai-usagebar", "usage", "--config"])).unwrap_err();
        assert_eq!(err, "--config requires a path");
    }

    /// Both `--config` forms must hand the exact platform bytes to the path
    /// (the `=` form used to go through `to_string_lossy`, which replaced
    /// undecodable bytes with U+FFFD and produced a false "file not found").
    #[cfg(unix)]
    #[test]
    fn both_forms_keep_undecodable_bytes_intact() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let equals = std::ffi::OsString::from_vec(b"--config=caf\xe9.toml".to_vec());
        let (_rest, config) =
            split_config_arg(vec![std::ffi::OsString::from("ai-usagebar"), equals]).unwrap();
        assert_eq!(
            config.expect("equals form parsed").as_os_str().as_bytes(),
            b"caf\xe9.toml"
        );
        let spaced = std::ffi::OsString::from_vec(b"caf\xe9.toml".to_vec());
        let (_rest, config) = split_config_arg(vec![
            std::ffi::OsString::from("ai-usagebar"),
            std::ffi::OsString::from("--config"),
            spaced,
        ])
        .unwrap();
        assert_eq!(
            config.expect("spaced form parsed").as_os_str().as_bytes(),
            b"caf\xe9.toml"
        );
    }

    #[cfg(windows)]
    #[test]
    fn equals_form_keeps_lone_surrogates_intact() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let mut wide: Vec<u16> = "--config=".encode_utf16().collect();
        wide.push(0xDC00); // lone low surrogate: not valid Unicode
        let arg = std::ffi::OsString::from_wide(&wide);
        let (_rest, config) =
            split_config_arg(vec![std::ffi::OsString::from("ai-usagebar"), arg]).unwrap();
        assert_eq!(
            config
                .expect("equals form parsed")
                .as_os_str()
                .encode_wide()
                .collect::<Vec<_>>(),
            vec![0xDC00u16]
        );
    }
}
