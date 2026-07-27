//! Waybar widget binary. The library does all the work — this is just the
//! tokio bootstrap + clap parse.

use ai_usagebar::widget::cli::{Cli, Command};
use ai_usagebar::widget::run::run;
use clap::Parser;

fn main() {
    let cli = Cli::parse();
    // Subcommands (e.g. `account add`) are plain CLI tools, not the widget —
    // handle them before the widget bootstrap and exit with their own code.
    if let Some(Command::Account { action }) = &cli.command {
        std::process::exit(ai_usagebar::account::run(action));
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
    let code = rt.block_on(run(cli));
    std::process::exit(code);
}
