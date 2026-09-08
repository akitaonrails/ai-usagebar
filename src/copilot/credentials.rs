//! Obtain a GitHub OAuth token from the official GitHub CLI without handling
//! its credential files ourselves.

use std::io;
use std::process::{Command, Stdio};

use crate::error::{AppError, Result};
use crate::vendor::vendor_secret_env_vars_to_remove;

/// Where the GitHub CLI records a completed login. Its existence is the
/// cheapest honest answer to "is Copilot signed in": the token itself only
/// comes back from `gh auth token`, and a status list that runs a subprocess
/// per provider is not a status list.
///
/// This follows `gh`'s own documented precedence exactly, because a path we
/// invent is a wrong answer on somebody's machine: `GH_CONFIG_DIR`, then
/// `$XDG_CONFIG_HOME/gh`, then `%AppData%\GitHub CLI` on Windows, then
/// `~/.config/gh`.
pub fn default_hosts_path() -> Result<std::path::PathBuf> {
    hosts_path_with(
        |name| std::env::var_os(name).filter(|value| !value.is_empty()),
        crate::cache::home_dir()?,
    )
}

/// Test seam for [`default_hosts_path`]: the environment and the home
/// directory are the production inputs, so they are injected rather than read.
pub fn hosts_path_with(
    environment: impl Fn(&str) -> Option<std::ffi::OsString>,
    home: std::path::PathBuf,
) -> Result<std::path::PathBuf> {
    if let Some(dir) = environment("GH_CONFIG_DIR") {
        return Ok(std::path::PathBuf::from(dir).join("hosts.yml"));
    }
    if let Some(dir) = environment("XDG_CONFIG_HOME") {
        return Ok(std::path::PathBuf::from(dir).join("gh").join("hosts.yml"));
    }
    if cfg!(windows)
        && let Some(dir) = environment("AppData")
    {
        return Ok(std::path::PathBuf::from(dir)
            .join("GitHub CLI")
            .join("hosts.yml"));
    }
    Ok(home.join(".config").join("gh").join("hosts.yml"))
}

/// The deliberately narrow process description used to obtain the current
/// GitHub CLI OAuth token. Keeping it data makes the subprocess boundary
/// inspectable in tests and prevents a shell from entering this path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhAuthTokenCommand {
    pub program: std::path::PathBuf,
    pub args: [&'static str; 2],
    pub env_remove: Vec<&'static str>,
}

impl GhAuthTokenCommand {
    /// `gh` has no canonical install path across distributions, so unlike
    /// `grok` it is looked up on `PATH` by default. That makes the binary an
    /// ambient choice, which is why `[copilot] gh_binary` exists: point it at
    /// the trusted executable and the lookup stops being ambient.
    pub fn standard(gh_binary: Option<&std::path::Path>) -> Self {
        Self {
            program: gh_binary.map_or_else(|| std::path::PathBuf::from("gh"), Into::into),
            args: ["auth", "token"],
            // `gh auth token` must use its saved OAuth login, rather than an
            // arbitrary provider token inherited from this process.
            env_remove: vendor_secret_env_vars_to_remove(&[]),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhAuthTokenOutput {
    pub success: bool,
    pub stdout: Vec<u8>,
}

/// Injectable command boundary. Tests supply a fake runner, so they never
/// execute `gh`, inspect a real GitHub config directory, or use ambient env.
pub trait GhAuthTokenRunner {
    fn run(&self, command: &GhAuthTokenCommand) -> io::Result<GhAuthTokenOutput>;
}

pub struct SystemGhAuthTokenRunner;

impl GhAuthTokenRunner for SystemGhAuthTokenRunner {
    fn run(&self, command: &GhAuthTokenCommand) -> io::Result<GhAuthTokenOutput> {
        let mut process = Command::new(&command.program);
        process
            .args(command.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for variable in &command.env_remove {
            process.env_remove(variable);
        }
        let output = process.output()?;
        Ok(GhAuthTokenOutput {
            success: output.status.success(),
            stdout: output.stdout,
        })
    }
}

pub fn resolve_with(
    runner: &impl GhAuthTokenRunner,
    gh_binary: Option<&std::path::Path>,
) -> Result<String> {
    let command = GhAuthTokenCommand::standard(gh_binary);
    let output = match runner.run(&command) {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(login_error(
                "GitHub CLI (`gh`) is not installed. Install it, then run",
            ));
        }
        Err(_) => return Err(login_error("GitHub CLI could not be started. Run")),
    };
    if !output.success {
        return Err(login_error("GitHub CLI is not logged in. Run"));
    }
    let token = String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| login_error("GitHub CLI returned no OAuth token. Run"))?;
    Ok(token)
}

fn login_error(prefix: &str) -> AppError {
    AppError::Credentials(format!(
        "GitHub Copilot: {prefix} `gh auth login --web`, then select GitHub Copilot as the primary provider in Settings."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeRunner {
        result: io::Result<GhAuthTokenOutput>,
        command: RefCell<Option<GhAuthTokenCommand>>,
    }

    impl GhAuthTokenRunner for FakeRunner {
        fn run(&self, command: &GhAuthTokenCommand) -> io::Result<GhAuthTokenOutput> {
            *self.command.borrow_mut() = Some(command.clone());
            self.result
                .as_ref()
                .map(Clone::clone)
                .map_err(|error| io::Error::new(error.kind(), "fake gh failure"))
        }
    }

    #[test]
    fn runs_only_fixed_gh_auth_token_command_and_returns_trimmed_token() {
        let runner = FakeRunner {
            result: Ok(GhAuthTokenOutput {
                success: true,
                stdout: b"test-github-oauth-token\n".to_vec(),
            }),
            command: RefCell::new(None),
        };

        assert_eq!(
            resolve_with(&runner, None).unwrap(),
            "test-github-oauth-token"
        );
        let command = runner.command.into_inner().unwrap();
        assert_eq!(command.program, std::path::Path::new("gh"));
        assert_eq!(command.args, ["auth", "token"]);
        assert!(command.env_remove.contains(&"ZAI_API_KEY"));
        assert!(command.env_remove.contains(&"GITHUB_COPILOT_TOKEN"));
        assert!(command.env_remove.contains(&"GH_TOKEN"));
        assert!(command.env_remove.contains(&"GITHUB_TOKEN"));
    }

    /// `gh` has no canonical path, so `PATH` is the sensible default — but it
    /// is still an ambient choice about which binary runs on every refresh.
    /// `[copilot] gh_binary` is how a user pins it, so the setting has to
    /// actually reach the spawned command.
    /// `gh`'s own precedence, in order. A path we invent instead is a wrong
    /// answer on somebody's machine: an XDG user's login would read as absent
    /// and report Copilot as never signed in.
    #[test]
    fn hosts_path_follows_the_github_cli_precedence() {
        use std::ffi::OsString;
        use std::path::PathBuf;
        let home = PathBuf::from("/home/u");
        let only = |wanted: &'static str, value: &'static str| {
            move |name: &str| (name == wanted).then(|| OsString::from(value))
        };

        assert_eq!(
            hosts_path_with(only("GH_CONFIG_DIR", "/cfg/gh"), home.clone()).unwrap(),
            PathBuf::from("/cfg/gh/hosts.yml"),
            "GH_CONFIG_DIR is used verbatim, with no `gh` segment appended"
        );
        assert_eq!(
            hosts_path_with(only("XDG_CONFIG_HOME", "/xdg"), home.clone()).unwrap(),
            PathBuf::from("/xdg/gh/hosts.yml")
        );
        assert_eq!(
            hosts_path_with(|_| None, home.clone()).unwrap(),
            home.join(".config").join("gh").join("hosts.yml"),
            "with nothing set, the home convention"
        );

        // GH_CONFIG_DIR outranks XDG_CONFIG_HOME.
        let both = |name: &str| match name {
            "GH_CONFIG_DIR" => Some(OsString::from("/cfg/gh")),
            "XDG_CONFIG_HOME" => Some(OsString::from("/xdg")),
            _ => None,
        };
        assert_eq!(
            hosts_path_with(both, home.clone()).unwrap(),
            PathBuf::from("/cfg/gh/hosts.yml")
        );

        // `AppData` is Windows-only and ranks below XDG, so it must not
        // capture a Linux or macOS machine that happens to have it set.
        let appdata =
            hosts_path_with(only("AppData", "C:/Users/u/AppData/Roaming"), home.clone()).unwrap();
        if cfg!(windows) {
            assert_eq!(
                appdata,
                PathBuf::from("C:/Users/u/AppData/Roaming/GitHub CLI/hosts.yml")
            );
        } else {
            assert_eq!(appdata, home.join(".config").join("gh").join("hosts.yml"));
        }
    }

    #[test]
    fn a_configured_gh_binary_replaces_the_path_lookup() {
        let pinned = std::path::Path::new("/opt/github/bin/gh");
        assert_eq!(
            GhAuthTokenCommand::standard(Some(pinned)).program,
            pinned,
            "a pinned binary must be used verbatim"
        );
        assert_eq!(
            GhAuthTokenCommand::standard(None).program,
            std::path::Path::new("gh"),
            "unset still means the PATH lookup"
        );
        // The rest of the boundary is fixed either way.
        for command in [
            GhAuthTokenCommand::standard(Some(pinned)),
            GhAuthTokenCommand::standard(None),
        ] {
            assert_eq!(command.args, ["auth", "token"]);
            assert!(command.env_remove.contains(&"GITHUB_TOKEN"));
        }
    }

    #[test]
    fn login_failure_never_echoes_gh_output() {
        let runner = FakeRunner {
            result: Ok(GhAuthTokenOutput {
                success: false,
                stdout: b"private-token-or-error".to_vec(),
            }),
            command: RefCell::new(None),
        };

        let error = resolve_with(&runner, None).unwrap_err().to_string();
        assert!(error.contains("gh auth login --web"));
        assert!(!error.contains("private-token-or-error"));
    }
}
