//! Device-code sign-in from the popover, without a terminal.
//!
//! Three providers sign in with a one-time code typed on their own page:
//! Codex (`codex login --device-auth`), Kimi (`kimi login`) and Nous Research
//! (this crate's own device flow, [`crate::nous::oauth`]). The popover's sheet
//! sends `start-sign-in`; the host runs the flow on a thread of its own and
//! publishes where it stands as the `sign_in` fact: the code once there is
//! one, the verdict when it ends. The two CLIs run as they are, so each keeps
//! writing its login where its own readers, and ours, look for it.
//!
//! The page address stays in the host. `open-sign-in-page` names the flow by
//! `seq`, and only an address on the provider's own hosts is ever kept.
//!
//! A newer `start-sign-in`, or a `cancel-sign-in`, replaces the fact. The flow
//! it replaced then finds another `seq`, stops its child and publishes
//! nothing. The whole process group is stopped, because Codex's npm launcher
//! runs the real binary as a child of its own.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::payload::{SharedFacts, SignInFact, SignInState, with_facts};
use crate::vendor::VendorId;

/// One provider's device-code flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    Codex,
    Kimi,
    Nous,
}

impl Flow {
    pub(crate) fn for_vendor(vendor: VendorId) -> Option<Flow> {
        match vendor {
            VendorId::Openai => Some(Flow::Codex),
            VendorId::Kimi => Some(Flow::Kimi),
            VendorId::NousResearch => Some(Flow::Nous),
            _ => None,
        }
    }

    /// The account the provider's page signs in, for the sheet's subtitle
    /// ("With your ChatGPT account").
    pub(crate) fn account(self) -> &'static str {
        match self {
            Flow::Codex => "ChatGPT",
            Flow::Kimi => "Kimi",
            Flow::Nous => "Nous Portal",
        }
    }

    /// Who hands out the code, for "Asking OpenAI for a sign-in code".
    pub(crate) fn issuer(self) -> &'static str {
        match self {
            Flow::Codex => "OpenAI",
            Flow::Kimi => "Kimi",
            Flow::Nous => "Nous Research",
        }
    }

    /// The CLI the flow runs; Nous runs in this process.
    fn command(self) -> &'static str {
        match self {
            Flow::Codex => "codex",
            Flow::Kimi => "kimi",
            Flow::Nous => "ai-usagebar",
        }
    }

    /// How long a code lives when the flow does not say. Codex prints its own
    /// fifteen minutes; Kimi prints its own seconds.
    fn default_expiry(self) -> Duration {
        Duration::from_secs(15 * 60)
    }

    /// The page that takes the code must be on the provider's own hosts:
    /// https, the default port, no credentials in the address.
    pub(crate) fn page_allowed(self, url: &str) -> bool {
        if self == Flow::Nous {
            return crate::nous::oauth::is_safe_portal_url(url);
        }
        let Ok(parsed) = reqwest::Url::parse(url) else {
            return false;
        };
        let domains: &[&str] = match self {
            Flow::Codex => &["openai.com", "chatgpt.com"],
            Flow::Kimi => &["kimi.com", "kimi.ai", "moonshot.cn", "moonshot.ai"],
            Flow::Nous => &[],
        };
        url.len() <= 2048
            && parsed.scheme() == "https"
            && parsed.port_or_known_default() == Some(443)
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.host_str().is_some_and(|host| {
                domains
                    .iter()
                    .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
            })
    }
}

/// How one flow runs. Production resolves the CLI and the config
/// ([`prepare`]); tests pass a script, or endpoints and a store, of their own.
pub(crate) enum Runner {
    Cli(CliLogin),
    Nous(NousLogin),
}

pub(crate) struct CliLogin {
    pub flow: Flow,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, OsString)>,
    /// How long the CLI gets to print its code.
    pub code_wait: Duration,
}

pub(crate) struct NousLogin {
    pub endpoints: crate::nous::oauth::Endpoints,
    pub store: crate::nous::credentials::CredentialStore,
    pub page_ok: fn(&str) -> bool,
}

/// What the flow's thread reports back to the host.
pub(crate) struct Hooks {
    /// The fact changed: push it to the popover.
    pub push: Box<dyn Fn() + Send>,
    /// The login landed for this vendor slug: drop its cache, re-read the
    /// catalog, fetch a report.
    pub signed_in: Box<dyn FnOnce(&str) + Send>,
}

/// How a flow ended.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Ok,
    Expired,
    Unavailable(&'static str),
    Failed {
        reason: &'static str,
        error: String,
    },
    /// A newer start or a cancel replaced it: publish nothing.
    Gone,
}

/// The loop's tick: how often a flow looks at its child and its fact.
const TICK: Duration = Duration::from_millis(200);
/// How long past its code's expiry a CLI may still run before it is stopped.
const GRACE: Duration = Duration::from_secs(30);
/// The output lines kept for the verdict.
const TAIL_LINES: usize = 40;
/// One output line's cap; a CLI that never prints a newline still yields.
const LINE_BYTES: u64 = 4096;
/// Lines buffered between the readers and the flow. A full buffer blocks the
/// reader, and the CLI behind it, until the flow catches up.
const CHANNEL_LINES: usize = 64;
/// Lines handled per tick before the flow looks at its fact and deadline.
const LINES_PER_TICK: usize = 256;
/// How long the last lines may keep coming once the CLI exited.
const DRAIN: Duration = Duration::from_secs(1);
/// The longest a code is believed to live, whatever the flow says.
const MAX_EXPIRY: Duration = Duration::from_secs(60 * 60);

/// `{"cmd":"start-sign-in","id":"openai","seq":3}`: open the flow for the
/// provider, unless a newer one is already on screen. `push` sends the fact
/// to the popover; `refresh` asks for a report once the login landed.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) fn start(
    facts: &SharedFacts,
    value: &Value,
    push: impl Fn() + Send + 'static,
    refresh: impl FnOnce() + Send + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    let hooks = Hooks {
        push: Box::new(push),
        signed_in: Box::new(move |slug| {
            // The reading on screen belongs to the login before this one.
            if let Ok(cache) = crate::cache::Cache::for_vendor(slug) {
                let _ = cache.try_forget();
            }
            super::providers::request();
            refresh();
        }),
    };
    start_with(facts, value, prepare, hooks)
}

/// The shared start: parse the request, publish `Starting`, then either the
/// refusal `prepare` returned or the flow's thread. A malformed request, a
/// provider without a device flow, or a `seq` no newer than the fact on
/// screen is ignored.
pub(crate) fn start_with(
    facts: &SharedFacts,
    value: &Value,
    prepare: impl FnOnce(Flow) -> Result<Runner, &'static str>,
    hooks: Hooks,
) -> Option<std::thread::JoinHandle<()>> {
    let (vendor, flow, seq) = start_request(value)?;
    let slug = vendor.slug().to_string();
    let mut accepted = false;
    with_facts(facts, |f| {
        if f.sign_in.as_ref().is_some_and(|current| current.seq >= seq) {
            return;
        }
        f.sign_in = Some(SignInFact {
            id: slug.clone(),
            seq,
            state: SignInState::Starting,
            code: String::new(),
            expires_at: 0,
            reason: "",
            error: String::new(),
            url: String::new(),
            committing: false,
        });
        accepted = true;
    });
    if !accepted {
        return None;
    }
    let job = Job {
        facts: std::sync::Arc::clone(facts),
        seq,
    };
    let runner = match prepare(flow) {
        Ok(runner) => runner,
        Err(reason) => {
            job.finish(&slug, Outcome::Unavailable(reason), hooks);
            return None;
        }
    };
    let spawned = std::thread::Builder::new()
        .name("ai-usagebar-sign-in".into())
        .spawn(move || {
            let outcome = match runner {
                Runner::Cli(login) => run_cli(&job, login, &*hooks.push),
                Runner::Nous(login) => run_nous(&job, login, &*hooks.push),
            };
            job.finish(&slug, outcome, hooks);
        });
    match spawned {
        Ok(handle) => Some(handle),
        Err(_) => {
            let job = Job {
                facts: std::sync::Arc::clone(facts),
                seq,
            };
            job.publish(|fact| {
                fact.state = SignInState::Failed;
                fact.error = "Could not start the sign-in worker.".into();
            });
            None
        }
    }
}

/// The request's (vendor, flow, seq), or `None` when it is not a well-formed
/// `start-sign-in` for a provider with a device flow.
fn start_request(value: &Value) -> Option<(VendorId, Flow, u64)> {
    let vendor = value
        .get("id")
        .and_then(Value::as_str)
        .and_then(VendorId::from_slug)?;
    let flow = Flow::for_vendor(vendor)?;
    let seq = value.get("seq").and_then(Value::as_u64)?;
    Some((vendor, flow, seq))
}

fn request_seq(value: &Value) -> Option<u64> {
    value.get("seq").and_then(Value::as_u64)
}

/// `{"cmd":"cancel-sign-in","seq":3}`: the sheet closed. Clears the fact when
/// it is that flow's, which stops the flow; `true` when it did. A flow that is
/// already saving the login it was granted is past cancelling: its fact stays,
/// and its verdict follows.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) fn cancel(facts: &SharedFacts, value: &Value) -> bool {
    let Some(seq) = request_seq(value) else {
        return false;
    };
    let mut cleared = false;
    with_facts(facts, |f| {
        if f.sign_in
            .as_ref()
            .is_some_and(|fact| fact.seq == seq && !fact.committing)
        {
            f.sign_in = None;
            cleared = true;
        }
    });
    cleared
}

/// The page `open-sign-in-page` asks for: the flow's own, only while its code
/// is on screen, and checked against the flow's hosts once more.
pub(crate) fn page_to_open(facts: &SharedFacts, value: &Value) -> Option<String> {
    let seq = request_seq(value)?;
    let fact = facts.lock().ok()?.sign_in.clone()?;
    let flow = VendorId::from_slug(&fact.id).and_then(Flow::for_vendor)?;
    (fact.seq == seq && fact.state == SignInState::Waiting && flow.page_allowed(&fact.url))
        .then_some(fact.url)
}

/// `{"cmd":"open-sign-in-page","seq":3}`. Opened without a shell: the address
/// is the provider's, and its `&` and `%` stay data.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) fn open_page(facts: &SharedFacts, value: &Value) {
    if let Some(url) = page_to_open(facts, value) {
        use crate::nous::oauth::BrowserOpener;
        let _ = crate::nous::oauth::SystemBrowserOpener.open(&url);
    }
}

/// The code `copy-sign-in-code` asks for, while the flow's code is on screen.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) fn code_to_copy(facts: &SharedFacts, value: &Value) -> Option<String> {
    let seq = request_seq(value)?;
    let fact = facts.lock().ok()?.sign_in.clone()?;
    (fact.seq == seq && fact.state == SignInState::Waiting && is_code(&fact.code))
        .then_some(fact.code)
}

/// One flow's handle on the fact: it may write only while the fact is still
/// its own.
struct Job {
    facts: SharedFacts,
    seq: u64,
}

impl Job {
    fn current(&self) -> bool {
        self.facts
            .lock()
            .ok()
            .and_then(|f| f.sign_in.as_ref().map(|fact| fact.seq == self.seq))
            .unwrap_or(false)
    }

    /// Edit the fact in one pass, only while it is this flow's; `true` when
    /// it was.
    fn publish(&self, edit: impl FnOnce(&mut SignInFact)) -> bool {
        let mut published = false;
        with_facts(&self.facts, |f| {
            if let Some(fact) = f.sign_in.as_mut().filter(|fact| fact.seq == self.seq) {
                edit(fact);
                published = true;
            }
        });
        published
    }

    fn finish(&self, slug: &str, outcome: Outcome, hooks: Hooks) {
        let landed = outcome == Outcome::Ok;
        let published = match outcome {
            Outcome::Gone => false,
            Outcome::Ok => self.publish(|fact| {
                fact.state = SignInState::Ok;
                fact.code.clear();
                fact.url.clear();
            }),
            Outcome::Expired => self.publish(|fact| {
                fact.state = SignInState::Expired;
                fact.url.clear();
            }),
            Outcome::Unavailable(reason) => self.publish(|fact| {
                fact.state = SignInState::Unavailable;
                fact.reason = reason;
                fact.code.clear();
                fact.url.clear();
            }),
            Outcome::Failed { reason, error } => self.publish(|fact| {
                fact.state = SignInState::Failed;
                fact.reason = reason;
                fact.error = error.chars().take(300).collect();
                fact.code.clear();
                fact.url.clear();
            }),
        };
        if published {
            (hooks.push)();
        }
        // A login that landed counts even when the sheet closed meanwhile:
        // the CLI exited on its own after saving it, and the card should
        // read it.
        if landed {
            (hooks.signed_in)(slug);
        }
    }

    /// The approval arrived: claim the right to save it, in the same pass
    /// that proves the flow is still on screen. From here `cancel` leaves the
    /// fact alone, so a cancel either lands before this and nothing is saved,
    /// or after it and the login is kept; never both.
    fn begin_commit(&self) -> bool {
        self.publish(|fact| fact.committing = true)
    }

    /// The code is on screen: publish it with its page and expiry.
    fn show_code(&self, code: &str, url: &str, expires_in: Duration) -> bool {
        let expires_at = now_ms().saturating_add(duration_ms(expires_in));
        self.publish(|fact| {
            fact.state = SignInState::Waiting;
            fact.code = code.to_string();
            fact.url = url.to_string();
            fact.expires_at = expires_at;
        })
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn duration_ms(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// The production preparation: the provider's CLI and the place its login
/// must land, from config.toml and the disk, or the reason it cannot run.
fn prepare(flow: Flow) -> Result<Runner, &'static str> {
    let config = crate::config::Config::load().unwrap_or_default();
    let home = crate::cache::home_dir().map_err(|_| "cli-missing")?;
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    match flow {
        Flow::Nous => Ok(Runner::Nous(NousLogin {
            endpoints: crate::nous::oauth::Endpoints::default(),
            store: crate::nous::credentials::CredentialStore::default(),
            page_ok: crate::nous::oauth::is_safe_portal_url,
        })),
        Flow::Codex => {
            let codex_home = codex_home_for(config.openai.codex_auth_path.as_deref(), &home)?;
            let program = find_cli("codex", &path_var, &home).ok_or("cli-missing")?;
            Ok(Runner::Cli(CliLogin {
                flow,
                env: vec![
                    ("CODEX_HOME".into(), codex_home.into_os_string()),
                    ("PATH".into(), child_path(&program, &path_var, &home)),
                    ("NO_COLOR".into(), "1".into()),
                ],
                program,
                args: vec!["login".into(), "--device-auth".into()],
                code_wait: Duration::from_secs(90),
            }))
        }
        Flow::Kimi => {
            let default = crate::kimi::oauth::default_home()
                .map(|kimi_home| crate::kimi::oauth::credentials_path_in(&kimi_home))
                .map_err(|_| "cli-missing")?;
            if config
                .kimi
                .credentials_path
                .as_ref()
                .is_some_and(|path| *path != default)
            {
                return Err("custom-path");
            }
            let program = find_cli("kimi", &path_var, &home).ok_or("cli-missing")?;
            Ok(Runner::Cli(CliLogin {
                flow,
                env: vec![
                    ("PATH".into(), child_path(&program, &path_var, &home)),
                    ("NO_COLOR".into(), "1".into()),
                ],
                program,
                args: kimi_args(&config.kimi.region),
                code_wait: Duration::from_secs(90),
            }))
        }
    }
}

/// `CODEX_HOME` for the login the Codex card reads: `~/.codex`, or the
/// directory of `[openai] codex_auth_path` when that file is the `auth.json`
/// Codex writes there. A file by any other name cannot be written by
/// `codex login`, so the terminal is the way left.
fn codex_home_for(auth_path: Option<&Path>, home: &Path) -> Result<PathBuf, &'static str> {
    match auth_path {
        None => Ok(home.join(".codex")),
        Some(path) if path.file_name().is_some_and(|name| name == "auth.json") => path
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .ok_or("custom-path"),
        Some(_) => Err("custom-path"),
    }
}

/// `kimi login`, pinned to the region config.toml reads, when it names one.
fn kimi_args(region: &str) -> Vec<String> {
    let mut args = vec!["login".to_string()];
    match crate::kimi::oauth::Region::parse(region) {
        Some(crate::kimi::oauth::Region::MainlandCn) => {
            args.extend(["--region".into(), "mainland-cn".into()]);
        }
        Some(crate::kimi::oauth::Region::Global) => {
            args.extend(["--region".into(), "global".into()]);
        }
        None => {}
    }
    args
}

/// Where a tray started by launchd or a desktop session looks for a CLI
/// beyond its own `PATH`, which on macOS is only the system's directories.
fn extra_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![
        home.join(".local").join("bin"),
        home.join(".bun").join("bin"),
        home.join(".volta").join("bin"),
        home.join(".npm-global").join("bin"),
    ];
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
    } else {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    dirs
}

/// The CLI on `PATH`, else in the directories installers use.
fn find_cli(name: &str, path_var: &std::ffi::OsStr, home: &Path) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        vec![format!("{name}.exe"), format!("{name}.cmd")]
    } else {
        vec![name.to_string()]
    };
    std::env::split_paths(path_var)
        .chain(extra_dirs(home))
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

/// The child's `PATH`: the CLI's own directory first, so an npm launcher's
/// `#!/usr/bin/env node` finds the `node` installed beside it, then the
/// installers' directories, then the tray's own.
fn child_path(program: &Path, path_var: &std::ffi::OsStr, home: &Path) -> OsString {
    let dirs = program
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .chain(extra_dirs(home))
        .chain(std::env::split_paths(path_var));
    std::env::join_paths(dirs).unwrap_or_else(|_| path_var.to_os_string())
}

/// Run a CLI's device login: read its output for the code and its page,
/// publish them, and wait for it to exit, for the code's expiry, or for the
/// fact to move on.
fn run_cli(job: &Job, login: CliLogin, push: &dyn Fn()) -> Outcome {
    let command_name = login.flow.command();
    let mut command = Command::new(&login.program);
    command
        .args(&login.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in &login.env {
        command.env(name, value);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(crate::process::CREATE_NO_WINDOW);
    }
    let mut tree = match Tree::spawn(&mut command) {
        Ok(tree) => tree,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Outcome::Unavailable("cli-missing");
        }
        Err(error) => {
            return Outcome::Failed {
                reason: "",
                error: format!("Could not start `{command_name}`: {error}"),
            };
        }
    };
    let (sender, lines) = mpsc::sync_channel(CHANNEL_LINES);
    if let Some(stdout) = tree.child.stdout.take() {
        read_lines(stdout, sender.clone());
    }
    if let Some(stderr) = tree.child.stderr.take() {
        read_lines(stderr, sender);
    } else {
        drop(sender);
    }

    let mut output = Parsed::default();
    let mut tail = std::collections::VecDeque::with_capacity(TAIL_LINES);
    let mut deadline = Instant::now() + login.code_wait;
    let mut shown: Option<Duration> = None;
    loop {
        // Every line already there, so a code and its expiry printed in one
        // write are read together; a bounded batch, so a flood cannot hold
        // off the fact and the deadline.
        if let Ok(first) = lines.recv_timeout(TICK) {
            for line in std::iter::once(first)
                .chain(lines.try_iter())
                .take(LINES_PER_TICK)
            {
                take_line(login.flow, &line, &mut output, &mut tail);
            }
        }
        if !job.current() {
            tree.stop();
            return Outcome::Gone;
        }
        if let (Some(code), Some(url)) = (&output.code, &output.url) {
            let expires_in = output
                .expires_in
                .unwrap_or_else(|| login.flow.default_expiry())
                .min(MAX_EXPIRY);
            if shown != Some(expires_in) {
                if !login.flow.page_allowed(url) {
                    tree.stop();
                    return Outcome::Failed {
                        reason: "",
                        error: format!(
                            "`{command_name}` named a sign-in page outside {}'s site.",
                            login.flow.issuer()
                        ),
                    };
                }
                if job.show_code(code, url, expires_in) {
                    push();
                }
                deadline = Instant::now() + expires_in + GRACE;
                shown = Some(expires_in);
            }
        }
        match tree.exited() {
            Ok(true) => {
                // The last lines, for at most `DRAIN`: a descendant that
                // keeps a pipe open cannot hold the flow.
                let until = Instant::now() + DRAIN;
                while let Some(left) = until
                    .checked_duration_since(Instant::now())
                    .filter(|left| !left.is_zero())
                {
                    match lines.recv_timeout(left.min(TICK)) {
                        Ok(line) => take_line(login.flow, &line, &mut output, &mut tail),
                        Err(mpsc::RecvTimeoutError::Timeout) if job.current() => {}
                        Err(_) => break,
                    }
                }
                // What the CLI left running goes with it.
                let success = tree.end().is_some_and(|status| status.success());
                return verdict(login.flow, success, &tail);
            }
            Ok(false) => {}
            Err(error) => {
                tree.stop();
                return Outcome::Failed {
                    reason: "",
                    error: format!("Lost track of `{command_name}`: {error}"),
                };
            }
        }
        if Instant::now() >= deadline {
            tree.stop();
            return if shown.is_some() {
                Outcome::Expired
            } else {
                Outcome::Failed {
                    reason: "",
                    error: format!("`{command_name}` did not print a sign-in code."),
                }
            };
        }
    }
}

fn take_line(
    flow: Flow,
    line: &str,
    output: &mut Parsed,
    tail: &mut std::collections::VecDeque<String>,
) {
    let line = strip_ansi(line);
    output.feed(flow, &line);
    if tail.len() == TAIL_LINES {
        tail.pop_front();
    }
    tail.push_back(line);
}

/// Forward a stream's lines, each cut at [`LINE_BYTES`], until it closes or
/// nobody listens.
fn read_lines(stream: impl Read + Send + 'static, sender: mpsc::SyncSender<String>) {
    let _ = std::thread::Builder::new()
        .name("ai-usagebar-sign-in-output".into())
        .spawn(move || {
            let mut reader = BufReader::new(stream);
            let mut buffer = Vec::new();
            loop {
                buffer.clear();
                match (&mut reader)
                    .take(LINE_BYTES)
                    .read_until(b'\n', &mut buffer)
                {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let line = String::from_utf8_lossy(&buffer).trim_end().to_string();
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                }
            }
        });
}

/// What a CLI's output has said so far.
#[derive(Debug, Default, PartialEq, Eq)]
struct Parsed {
    code: Option<String>,
    url: Option<String>,
    expires_in: Option<Duration>,
    /// Codex prints the code on the line after its "one-time code" line.
    code_next: bool,
}

impl Parsed {
    fn feed(&mut self, flow: Flow, line: &str) {
        let text = line.trim();
        match flow {
            // 1. Open this link in your browser and sign in to your account
            //    https://auth.openai.com/codex/device
            // 2. Enter this one-time code (expires in 15 minutes)
            //    ABCD-12345
            Flow::Codex => {
                if self.code_next && !text.is_empty() {
                    self.code_next = false;
                    if self.code.is_none() && is_code(text) {
                        self.code = Some(text.to_string());
                    }
                    return;
                }
                if text.contains("one-time code") {
                    self.code_next = true;
                    if let Some(minutes) = number_after(text, "expires in ")
                        && text.contains("minute")
                    {
                        self.expires_in = Some(Duration::from_secs(minutes.saturating_mul(60)));
                    }
                    return;
                }
                if self.url.is_none() {
                    self.url = first_url(text);
                }
            }
            // Opening browser for Kimi device login: https://…
            // If the browser did not open, paste the URL above and enter code: ABCD-1234
            // Code expires in 600s.
            Flow::Kimi => {
                if let Some(rest) = text.strip_prefix("Opening browser for Kimi device login:")
                    && self.url.is_none()
                {
                    self.url = first_url(rest);
                }
                if let Some((_, code)) = text.split_once("enter code:") {
                    let code = code.trim();
                    if self.code.is_none() && is_code(code) {
                        self.code = Some(code.to_string());
                    }
                }
                if text.starts_with("Code expires in ")
                    && let Some(seconds) = number_after(text, "Code expires in ")
                {
                    self.expires_in = Some(Duration::from_secs(seconds));
                }
            }
            Flow::Nous => {}
        }
    }
}

/// A one-time code as the providers print it: letters, digits and dashes.
fn is_code(text: &str) -> bool {
    (4..=24).contains(&text.len())
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        && text.chars().any(|ch| ch.is_ascii_alphanumeric())
}

fn first_url(text: &str) -> Option<String> {
    let start = text.find("https://")?;
    let url: String = text[start..]
        .chars()
        .take_while(|ch| !ch.is_whitespace())
        .collect();
    Some(url)
}

/// The integer right after `marker` in `text`.
fn number_after(text: &str, marker: &str) -> Option<u64> {
    let (_, rest) = text.split_once(marker)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Drop terminal escape sequences: Codex colors its prompt even with
/// `NO_COLOR` set.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

/// The verdict of a CLI that exited on its own.
fn verdict(flow: Flow, success: bool, tail: &std::collections::VecDeque<String>) -> Outcome {
    if success {
        return Outcome::Ok;
    }
    let text = tail
        .iter()
        .map(|line| line.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("\n");
    match flow {
        Flow::Codex if text.contains("device code login is not enabled") => {
            return Outcome::Unavailable("device-off");
        }
        Flow::Codex if text.contains("timed out") => return Outcome::Expired,
        Flow::Kimi if text.contains("login failed") && text.contains("expired") => {
            return Outcome::Expired;
        }
        Flow::Kimi if text.contains("login cancelled") => {
            return Outcome::Failed {
                reason: "denied",
                error: last_error_line(tail, flow),
            };
        }
        _ => {}
    }
    Outcome::Failed {
        reason: "",
        error: last_error_line(tail, flow),
    }
}

/// The line that says what went wrong: the last one naming an error or a
/// failure, else the last one printed.
fn last_error_line(tail: &std::collections::VecDeque<String>, flow: Flow) -> String {
    let lines = || {
        tail.iter()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
    };
    lines()
        .rev()
        .find(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("error") || lower.contains("failed") || lower.contains("cancelled")
        })
        .or_else(|| lines().next_back())
        .map(crate::display::sanitize_untrusted_line)
        .unwrap_or_else(|| format!("`{}` exited without signing in.", flow.command()))
}

/// A CLI and what it starts, owned until it is reaped.
///
/// On Unix the CLI leads a process group of its own. The group is signalled
/// only while the leader is registered in [`RUNNING`] and unreaped, so its id
/// cannot have been handed to anyone else: exits are watched without reaping
/// (`waitid` with `WNOWAIT`), and the leader leaves the registry, under its
/// lock, before it is reaped. On Windows the process handle stays open until
/// then, which keeps the id from being reused, and the tree is ended by
/// `taskkill /T`. No job object: `kimi login` opens the browser through
/// `Start-Process`, and a job would take the browser down with the CLI.
struct Tree {
    child: Child,
    ended: bool,
    registry: &'static std::sync::Mutex<Running>,
}

impl Tree {
    fn spawn(command: &mut Command) -> std::io::Result<Tree> {
        Tree::spawn_in(&RUNNING, command)
    }

    /// Spawned and registered under the registry's lock, so `stop_all` can
    /// never miss a CLI started while the tray quits; once it has run, no
    /// CLI starts at all.
    fn spawn_in(
        registry: &'static std::sync::Mutex<Running>,
        command: &mut Command,
    ) -> std::io::Result<Tree> {
        let mut running = lock(registry);
        if running.closed {
            return Err(std::io::Error::other("the tray is quitting"));
        }
        let child = command.spawn()?;
        running.pids.push(child.id());
        Ok(Tree {
            child,
            ended: false,
            registry,
        })
    }

    /// Whether the CLI exited; on Unix without reaping it.
    fn exited(&mut self) -> std::io::Result<bool> {
        #[cfg(unix)]
        {
            use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
            waitid(
                WaitId::Pid(Pid::from_child(&self.child)),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            )
            .map(|status| status.is_some())
            .map_err(std::io::Error::from)
        }
        #[cfg(not(unix))]
        {
            self.child.try_wait().map(|status| status.is_some())
        }
    }

    /// Signal everything the CLI started; `gently` gives it a moment first.
    fn signal(&mut self, gently: bool) {
        #[cfg(unix)]
        {
            use rustix::process::{Pid, Signal, kill_process_group};
            let pid = Pid::from_child(&self.child);
            if gently {
                let _ = kill_process_group(pid, Signal::TERM);
                std::thread::sleep(Duration::from_millis(300));
            }
            let _ = kill_process_group(pid, Signal::KILL);
        }
        #[cfg(windows)]
        {
            let _ = gently;
            kill_tree(self.child.id());
        }
        let _ = self.child.kill();
    }

    /// Stop a CLI that is still at work, and reap it.
    fn stop(mut self) {
        self.signal(true);
        self.end();
    }

    /// End a CLI that exited: whatever it left in its group goes with it,
    /// then it leaves the registry and is reaped, in that order.
    fn end(mut self) -> Option<std::process::ExitStatus> {
        self.finish()
    }

    fn finish(&mut self) -> Option<std::process::ExitStatus> {
        if self.ended {
            return None;
        }
        self.ended = true;
        self.signal(false);
        let pid = self.child.id();
        lock(self.registry).pids.retain(|running| *running != pid);
        self.child.wait().ok()
    }
}

impl Drop for Tree {
    /// A flow that unwinds still takes its CLI along, and never leaves an id
    /// behind in the registry.
    fn drop(&mut self) {
        self.finish();
    }
}

/// `taskkill /T /F` on a process and its children, from System32 by full
/// path rather than by a `PATH` lookup.
#[cfg(windows)]
fn kill_tree(pid: u32) {
    use std::os::windows::process::CommandExt;
    let taskkill = std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join("System32").join("taskkill.exe"))
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32\taskkill.exe"));
    let _ = Command::new(taskkill)
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(crate::process::CREATE_NO_WINDOW)
        .status();
}

/// The CLIs running right now, so a tray that quits can take them along
/// (a Codex login left behind would keep polling for fifteen minutes), and
/// whether it is quitting, so a flow that had not started its CLI yet does
/// not start it afterwards.
struct Running {
    closed: bool,
    pids: Vec<u32>,
}

static RUNNING: std::sync::Mutex<Running> = std::sync::Mutex::new(Running {
    closed: false,
    pids: Vec::new(),
});

/// A registry, whole even when a flow panicked while holding it.
fn lock(registry: &std::sync::Mutex<Running>) -> std::sync::MutexGuard<'_, Running> {
    registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Kill every sign-in CLI still running, and start no other; the hosts call
/// it on their way out.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) fn stop_all() {
    stop_all_in(&RUNNING);
}

/// The signals go out under the registry's lock: a CLI still listed has not
/// been reaped, and its flow cannot reap it until the lock is free.
fn stop_all_in(registry: &std::sync::Mutex<Running>) {
    let mut running = lock(registry);
    running.closed = true;
    for pid in running.pids.drain(..) {
        #[cfg(unix)]
        if let Some(pid) = i32::try_from(pid)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        #[cfg(windows)]
        kill_tree(pid);
    }
}

/// Nous Research's device flow, in this process: the same requests
/// `ai-usagebar auth nous login` makes, with the code published instead of
/// printed.
fn run_nous(job: &Job, login: NousLogin, push: &dyn Fn()) -> Outcome {
    use crate::nous::oauth::{self, OAuthError};
    let failed = |error: String| Outcome::Failed { reason: "", error };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return failed("Could not start the sign-in worker.".into());
    };
    runtime.block_on(async {
        let Ok(client) = reqwest::Client::builder()
            .timeout(crate::vendor::HTTP_CLIENT_TIMEOUT)
            .redirect(crate::vendor::same_origin_redirect_policy())
            .build()
        else {
            return failed("Could not initialize the HTTP client.".into());
        };
        let device = tokio::select! {
            device = oauth::request_device_code(&client, &login.endpoints) => device,
            () = replaced(job) => return Outcome::Gone,
        };
        let device = match device {
            Ok(device) => device,
            Err(error) => return failed(format!("Nous Research: {error}")),
        };
        if !(login.page_ok)(&device.verification_uri_complete) {
            return failed("Nous Research named a sign-in page outside its portal.".into());
        }
        // Shown, copied and typed elsewhere: only a code's shape goes out.
        if !is_code(&device.user_code) {
            return failed("Nous Research sent a sign-in code this sheet cannot show.".into());
        }
        // The poll gives up when the sheet says the code is gone.
        let mut device = device;
        device.expires_in = device.expires_in.min(MAX_EXPIRY.as_secs());
        let expires_in = Duration::from_secs(device.expires_in);
        if !job.show_code(
            &device.user_code,
            &device.verification_uri_complete,
            expires_in,
        ) {
            return Outcome::Gone;
        }
        push();
        let token = tokio::select! {
            token = oauth::poll_for_token(&client, &login.endpoints.token, &device) => token,
            () = replaced(job) => return Outcome::Gone,
        };
        match token {
            Err(OAuthError::ExpiredToken | OAuthError::Deadline) => Outcome::Expired,
            Err(OAuthError::AccessDenied) => Outcome::Failed {
                reason: "denied",
                error: OAuthError::AccessDenied.to_string(),
            },
            Err(error) => failed(format!("Nous Research: {error}")),
            // Approved after the sheet closed: the user cancelled, so the
            // token is dropped rather than saved.
            Ok(_) if !job.begin_commit() => Outcome::Gone,
            Ok(token) => oauth::credential_from_token(token, chrono::Utc::now())
                .and_then(|credential| oauth::persist_credential(&login.store, credential))
                .map_or_else(
                    |error| failed(format!("Nous Research: {error}")),
                    |()| Outcome::Ok,
                ),
        }
    })
}

/// Resolves once the fact is no longer this flow's.
async fn replaced(job: &Job) {
    while job.current() {
        tokio::time::sleep(TICK).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray::payload::HostFacts;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn shared() -> SharedFacts {
        Arc::new(Mutex::new(HostFacts::default()))
    }

    /// Hooks that count pushes and record the slug a landed login names.
    fn recording() -> (Hooks, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
        let pushes = Arc::new(AtomicUsize::new(0));
        let landed = Arc::new(Mutex::new(Vec::new()));
        let hooks = Hooks {
            push: Box::new({
                let pushes = Arc::clone(&pushes);
                move || {
                    pushes.fetch_add(1, Ordering::SeqCst);
                }
            }),
            signed_in: Box::new({
                let landed = Arc::clone(&landed);
                move |slug| landed.lock().unwrap().push(slug.to_string())
            }),
        };
        (hooks, pushes, landed)
    }

    fn quiet() -> Hooks {
        recording().0
    }

    fn fact(facts: &SharedFacts) -> Option<SignInFact> {
        facts.lock().unwrap().sign_in.clone()
    }

    /// Wait (bounded) for the fact to satisfy `done`.
    fn wait_for(
        facts: &SharedFacts,
        done: impl Fn(Option<&SignInFact>) -> bool,
    ) -> Option<SignInFact> {
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            let current = fact(facts);
            if done(current.as_ref()) {
                return current;
            }
            assert!(Instant::now() < until, "timed out; fact: {current:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    const CODEX_PROMPT: &str = "\nWelcome to Codex [v\u{1b}[90m0.164.0\u{1b}[0m]\n\
\u{1b}[90mOpenAI's command-line coding agent\u{1b}[0m\n\n\
Follow these steps to sign in with ChatGPT using device code authorization:\n\n\
1. Open this link in your browser and sign in to your account\n   \
\u{1b}[94mhttps://auth.openai.com/codex/device\u{1b}[0m\n\n\
2. Enter this one-time code \u{1b}[90m(expires in 15 minutes)\u{1b}[0m\n   \
\u{1b}[94mABCD-12345\u{1b}[0m\n\n\
\u{1b}[90mContinue only if you started this login in Codex. If a website or another person gave you this code, cancel.\u{1b}[0m\n";

    const KIMI_PROMPT: &str = "\nOpening browser for Kimi device login: https://www.kimi.com/code/authorize_device?user_code=WXYZ-1234\n\
If the browser did not open, paste the URL above and enter code: WXYZ-1234\n\
Code expires in 600s.\n\
Waiting for authorization to complete…\n";

    fn parse(flow: Flow, text: &str) -> Parsed {
        let mut parsed = Parsed::default();
        for line in text.lines() {
            parsed.feed(flow, &strip_ansi(line));
        }
        parsed
    }

    #[test]
    fn codex_output_yields_its_code_page_and_fifteen_minutes() {
        let parsed = parse(Flow::Codex, CODEX_PROMPT);
        assert_eq!(parsed.code.as_deref(), Some("ABCD-12345"));
        assert_eq!(
            parsed.url.as_deref(),
            Some("https://auth.openai.com/codex/device")
        );
        assert_eq!(parsed.expires_in, Some(Duration::from_secs(900)));
    }

    #[test]
    fn kimi_output_yields_its_code_page_and_seconds() {
        let parsed = parse(Flow::Kimi, KIMI_PROMPT);
        assert_eq!(parsed.code.as_deref(), Some("WXYZ-1234"));
        assert_eq!(
            parsed.url.as_deref(),
            Some("https://www.kimi.com/code/authorize_device?user_code=WXYZ-1234")
        );
        assert_eq!(parsed.expires_in, Some(Duration::from_secs(600)));
    }

    #[test]
    fn output_without_a_code_shape_yields_no_code() {
        let parsed = parse(
            Flow::Codex,
            "2. Enter this one-time code\n   rm -rf / && echo pwned\n",
        );
        assert_eq!(parsed.code, None);
        let parsed = parse(Flow::Kimi, "enter code: <script>\n");
        assert_eq!(parsed.code, None);
    }

    #[test]
    fn pages_must_be_on_the_providers_own_hosts() {
        assert!(Flow::Codex.page_allowed("https://auth.openai.com/codex/device"));
        assert!(Flow::Codex.page_allowed("https://chatgpt.com/device"));
        assert!(
            Flow::Kimi.page_allowed("https://www.kimi.com/code/authorize_device?user_code=A&b=1")
        );
        assert!(Flow::Kimi.page_allowed("https://auth.kimi.ai/device"));
        assert!(Flow::Nous.page_allowed("https://portal.nousresearch.com/device?user_code=X"));
        for url in [
            "http://auth.openai.com/codex/device",
            "https://auth.openai.com.evil.example/codex/device",
            "https://evilopenai.com/device",
            "https://user@auth.openai.com/device",
            "https://auth.openai.com:8443/device",
            "javascript:alert(1)",
            "https://kimi.com/device",
        ] {
            assert!(!Flow::Codex.page_allowed(url), "{url}");
        }
        assert!(!Flow::Kimi.page_allowed("https://auth.openai.com/codex/device"));
        assert!(!Flow::Nous.page_allowed("https://evil.example/device"));
    }

    #[test]
    fn verdicts_name_the_way_a_cli_ended() {
        let tail = |text: &str| text.lines().map(String::from).collect();
        assert_eq!(verdict(Flow::Codex, true, &tail("")), Outcome::Ok);
        assert_eq!(
            verdict(
                Flow::Codex,
                false,
                &tail(
                    "Error logging in with device code: device code login is not enabled for this Codex server. Use the browser login or verify the server URL."
                )
            ),
            Outcome::Unavailable("device-off")
        );
        assert_eq!(
            verdict(
                Flow::Codex,
                false,
                &tail("Error logging in with device code: device auth timed out after 15 minutes")
            ),
            Outcome::Expired
        );
        assert_eq!(
            verdict(
                Flow::Kimi,
                false,
                &tail("Login failed: device code expired")
            ),
            Outcome::Expired
        );
        assert_eq!(
            verdict(Flow::Kimi, false, &tail("Login cancelled: access denied")),
            Outcome::Failed {
                reason: "denied",
                error: "Login cancelled: access denied".into()
            }
        );
        assert_eq!(
            verdict(
                Flow::Codex,
                false,
                &tail("noise\nError logging in with device code: boom\n")
            ),
            Outcome::Failed {
                reason: "",
                error: "Error logging in with device code: boom".into()
            }
        );
        assert_eq!(
            verdict(Flow::Kimi, false, &tail("")),
            Outcome::Failed {
                reason: "",
                error: "`kimi` exited without signing in.".into()
            }
        );
    }

    #[test]
    fn codex_home_follows_the_auth_file_the_card_reads() {
        let home = Path::new("/home/u");
        assert_eq!(codex_home_for(None, home), Ok(home.join(".codex")));
        assert_eq!(
            codex_home_for(Some(Path::new("/x/work/auth.json")), home),
            Ok(PathBuf::from("/x/work"))
        );
        assert_eq!(
            codex_home_for(Some(Path::new("/x/work/codex.json")), home),
            Err("custom-path")
        );
        assert_eq!(
            codex_home_for(Some(Path::new("auth.json")), home),
            Err("custom-path")
        );
    }

    #[test]
    fn kimi_login_is_pinned_to_the_configured_region() {
        assert_eq!(kimi_args("auto"), vec!["login"]);
        assert_eq!(kimi_args("cn"), vec!["login", "--region", "mainland-cn"]);
        assert_eq!(kimi_args("global"), vec!["login", "--region", "global"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_cli_is_found_on_path_or_in_the_installers_directories() {
        let root = tempfile::TempDir::new().unwrap();
        let home = root.path().join("home");
        let on_path = root.path().join("bin");
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        std::fs::create_dir_all(&on_path).unwrap();
        std::fs::write(home.join(".local/bin/kimi"), "").unwrap();
        std::fs::write(on_path.join("codex"), "").unwrap();
        let path_var = std::env::join_paths([&on_path]).unwrap();
        assert_eq!(
            find_cli("codex", &path_var, &home),
            Some(on_path.join("codex"))
        );
        assert_eq!(
            find_cli("kimi", &path_var, &home),
            Some(home.join(".local/bin/kimi"))
        );
        assert_eq!(find_cli("absent", &path_var, &home), None);
        // The CLI's own directory leads the child's PATH, for `env node`.
        let child = child_path(&home.join(".local/bin/kimi"), &path_var, &home);
        let first = std::env::split_paths(&child).next().unwrap();
        assert_eq!(first, home.join(".local/bin"));
    }

    #[test]
    fn a_start_for_another_provider_or_without_a_seq_is_ignored() {
        let facts = shared();
        for request in [
            json!({"cmd": "start-sign-in", "id": "zai", "seq": 1}),
            json!({"cmd": "start-sign-in", "id": "anthropic", "seq": 1}),
            json!({"cmd": "start-sign-in", "id": "openai"}),
            json!({"cmd": "start-sign-in", "id": 7, "seq": 1}),
        ] {
            let handle = start_with(&facts, &request, |_| unreachable!(), quiet());
            assert!(handle.is_none());
            assert_eq!(fact(&facts), None);
        }
    }

    #[test]
    fn a_refused_preparation_says_why_without_a_thread() {
        let facts = shared();
        let (hooks, pushes, landed) = recording();
        let handle = start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "openai", "seq": 4}),
            |flow| {
                assert_eq!(flow, Flow::Codex);
                Err("custom-path")
            },
            hooks,
        );
        assert!(handle.is_none());
        let fact = fact(&facts).unwrap();
        assert_eq!(fact.id, "openai");
        assert_eq!(fact.seq, 4);
        assert_eq!(fact.state, SignInState::Unavailable);
        assert_eq!(fact.reason, "custom-path");
        assert_eq!(pushes.load(Ordering::SeqCst), 1);
        assert!(landed.lock().unwrap().is_empty());
    }

    #[test]
    fn an_older_start_never_replaces_the_flow_on_screen() {
        let facts = shared();
        start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "kimi", "seq": 9}),
            |_| Err("cli-missing"),
            quiet(),
        );
        let handle = start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "openai", "seq": 8}),
            |_| unreachable!(),
            quiet(),
        );
        assert!(handle.is_none());
        let fact = fact(&facts).unwrap();
        assert_eq!((fact.id.as_str(), fact.seq), ("kimi", 9));
    }

    fn waiting(facts: &SharedFacts, seq: u64, url: &str) {
        with_facts(facts, |f| {
            f.sign_in = Some(SignInFact {
                id: "openai".into(),
                seq,
                state: SignInState::Waiting,
                code: "ABCD-12345".into(),
                expires_at: 1,
                reason: "",
                error: String::new(),
                url: url.into(),
                committing: false,
            });
        });
    }

    #[test]
    fn the_page_and_the_code_go_out_only_for_the_flow_on_screen() {
        let facts = shared();
        waiting(&facts, 3, "https://auth.openai.com/codex/device");
        let mine = json!({"seq": 3});
        let other = json!({"seq": 2});
        assert_eq!(
            page_to_open(&facts, &mine).as_deref(),
            Some("https://auth.openai.com/codex/device")
        );
        assert_eq!(code_to_copy(&facts, &mine).as_deref(), Some("ABCD-12345"));
        assert_eq!(page_to_open(&facts, &other), None);
        assert_eq!(code_to_copy(&facts, &other), None);
        assert_eq!(page_to_open(&facts, &json!({})), None);
        // A page the allowlist refuses is never opened, whoever stored it.
        waiting(&facts, 3, "https://evil.example/device");
        assert_eq!(page_to_open(&facts, &mine), None);
        // Once the flow ended, neither goes out.
        with_facts(&facts, |f| {
            f.sign_in.as_mut().unwrap().state = SignInState::Expired;
        });
        assert_eq!(code_to_copy(&facts, &mine), None);
    }

    #[test]
    fn cancel_clears_only_its_own_flow() {
        let facts = shared();
        waiting(&facts, 3, "https://auth.openai.com/codex/device");
        assert!(!cancel(&facts, &json!({"seq": 2})));
        assert!(fact(&facts).is_some());
        assert!(cancel(&facts, &json!({"seq": 3})));
        assert_eq!(fact(&facts), None);
    }

    #[test]
    fn the_payload_carries_the_code_but_never_the_page() {
        let host = HostFacts {
            sign_in: Some(SignInFact {
                id: "openai".into(),
                seq: 3,
                state: SignInState::Waiting,
                code: "ABCD-12345".into(),
                expires_at: 1_700_000_000_000,
                reason: "",
                error: String::new(),
                url: "https://auth.openai.com/codex/device".into(),
                committing: false,
            }),
            ..HostFacts::default()
        };
        let payload = crate::tray::payload::wrap_report("{}", &host, 0, None);
        let sign_in = &payload["sign_in"];
        assert_eq!(sign_in["state"], "waiting");
        assert_eq!(sign_in["code"], "ABCD-12345");
        assert_eq!(sign_in["expires_at"], 1_700_000_000_000_i64);
        assert!(sign_in.get("url").is_none());
        assert!(!payload.to_string().contains("auth.openai.com"));
    }

    /// A script standing in for a CLI.
    #[cfg(unix)]
    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("cli");
        crate::test_support::write_script(&path, body);
        path
    }

    #[cfg(unix)]
    fn cli(flow: Flow, program: PathBuf) -> Runner {
        Runner::Cli(CliLogin {
            flow,
            program,
            args: Vec::new(),
            env: Vec::new(),
            code_wait: Duration::from_secs(10),
        })
    }

    /// The prompt as `printf` reproduces it, escapes and all.
    #[cfg(unix)]
    fn printf_codex_prompt() -> String {
        format!(
            "printf '%b' '{}'",
            CODEX_PROMPT
                .replace('\u{1b}', "\\033")
                .replace('\n', "\\n")
                .replace('\'', "")
        )
    }

    #[cfg(unix)]
    #[test]
    fn a_codex_login_shows_its_code_then_lands() {
        let root = tempfile::TempDir::new().unwrap();
        let program = script(
            root.path(),
            &format!(
                "{}\nsleep 1\necho 'Successfully logged in' >&2\nexit 0",
                printf_codex_prompt()
            ),
        );
        let facts = shared();
        let (hooks, pushes, landed) = recording();
        let handle = start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "openai", "seq": 1}),
            |flow| Ok(cli(flow, program)),
            hooks,
        )
        .expect("a thread");
        assert_eq!(fact(&facts).unwrap().state, SignInState::Starting);
        let shown = wait_for(&facts, |f| {
            f.is_some_and(|f| f.state == SignInState::Waiting)
        });
        let shown = shown.unwrap();
        assert_eq!(shown.code, "ABCD-12345");
        assert_eq!(shown.url, "https://auth.openai.com/codex/device");
        let left = shown.expires_at - now_ms();
        assert!((14 * 60 * 1000..=15 * 60 * 1000).contains(&left), "{left}");
        handle.join().unwrap();
        let done = fact(&facts).unwrap();
        assert_eq!(done.state, SignInState::Ok);
        assert!(done.code.is_empty() && done.url.is_empty());
        assert_eq!(*landed.lock().unwrap(), vec!["openai".to_string()]);
        assert!(pushes.load(Ordering::SeqCst) >= 2);
    }

    /// Whether `pid` is gone (reaped, or never was).
    #[cfg(unix)]
    fn gone(pid: i32) -> bool {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let alive = rustix::process::Pid::from_raw(pid)
                .is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok());
            if !alive {
                return true;
            }
            if Instant::now() >= until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[cfg(unix)]
    #[test]
    fn cancel_stops_the_cli_and_what_it_started() {
        let root = tempfile::TempDir::new().unwrap();
        let pid_file = root.path().join("child.pid");
        let program = script(
            root.path(),
            &format!(
                "{}\nsleep 60 &\necho $! > '{}'\nwait",
                printf_codex_prompt(),
                pid_file.display()
            ),
        );
        let facts = shared();
        let (hooks, _, landed) = recording();
        let handle = start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "openai", "seq": 5}),
            |flow| Ok(cli(flow, program)),
            hooks,
        )
        .unwrap();
        wait_for(&facts, |f| {
            f.is_some_and(|f| f.state == SignInState::Waiting)
        });
        let until = Instant::now() + Duration::from_secs(5);
        while !pid_file.exists() {
            assert!(Instant::now() < until, "the script never started its child");
            std::thread::sleep(Duration::from_millis(20));
        }
        let child: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(cancel(&facts, &json!({"seq": 5})));
        handle.join().unwrap();
        assert_eq!(fact(&facts), None, "a cancelled flow publishes nothing");
        assert!(gone(child), "the CLI's own child outlived the cancel");
        assert!(landed.lock().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_newer_start_replaces_the_running_flow() {
        let root = tempfile::TempDir::new().unwrap();
        let slow_dir = root.path().join("slow");
        let fast_dir = root.path().join("fast");
        std::fs::create_dir_all(&slow_dir).unwrap();
        std::fs::create_dir_all(&fast_dir).unwrap();
        let slow = script(
            &slow_dir,
            &format!("{}\nexec sleep 60", printf_codex_prompt()),
        );
        let fast = script(&fast_dir, &format!("{}\nexit 0", printf_codex_prompt()));
        let facts = shared();
        let first = start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "openai", "seq": 1}),
            |flow| Ok(cli(flow, slow)),
            quiet(),
        )
        .unwrap();
        wait_for(&facts, |f| {
            f.is_some_and(|f| f.state == SignInState::Waiting)
        });
        let second = start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "openai", "seq": 2}),
            |flow| Ok(cli(flow, fast)),
            quiet(),
        )
        .unwrap();
        first.join().unwrap();
        second.join().unwrap();
        let done = fact(&facts).unwrap();
        assert_eq!((done.seq, done.state), (2, SignInState::Ok));
    }

    #[cfg(unix)]
    #[test]
    fn device_sign_in_turned_off_is_unavailable() {
        let root = tempfile::TempDir::new().unwrap();
        let program = script(
            root.path(),
            "echo 'Error logging in with device code: device code login is not enabled for this Codex server. Use the browser login or verify the server URL.' >&2\nexit 1",
        );
        let facts = shared();
        let (hooks, _, landed) = recording();
        start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "openai", "seq": 1}),
            |flow| Ok(cli(flow, program)),
            hooks,
        )
        .unwrap()
        .join()
        .unwrap();
        let done = fact(&facts).unwrap();
        assert_eq!(done.state, SignInState::Unavailable);
        assert_eq!(done.reason, "device-off");
        assert!(landed.lock().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_cli_that_prints_no_code_is_stopped() {
        let root = tempfile::TempDir::new().unwrap();
        let program = script(root.path(), "exec sleep 60");
        let facts = shared();
        start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "kimi", "seq": 1}),
            |flow| {
                Ok(Runner::Cli(CliLogin {
                    flow,
                    program,
                    args: Vec::new(),
                    env: Vec::new(),
                    code_wait: Duration::from_millis(400),
                }))
            },
            quiet(),
        )
        .unwrap()
        .join()
        .unwrap();
        let done = fact(&facts).unwrap();
        assert_eq!(done.state, SignInState::Failed);
        assert_eq!(done.error, "`kimi` did not print a sign-in code.");
    }

    #[cfg(unix)]
    #[test]
    fn a_page_off_the_providers_site_ends_the_flow() {
        let root = tempfile::TempDir::new().unwrap();
        let program = script(
            root.path(),
            "echo 'Opening browser for Kimi device login: https://evil.example/device'\necho 'If the browser did not open, paste the URL above and enter code: WXYZ-1234'\nexec sleep 60",
        );
        let facts = shared();
        start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "kimi", "seq": 1}),
            |flow| Ok(cli(flow, program)),
            quiet(),
        )
        .unwrap()
        .join()
        .unwrap();
        let done = fact(&facts).unwrap();
        assert_eq!(done.state, SignInState::Failed);
        assert!(done.error.contains("outside Kimi's site"), "{}", done.error);
        assert!(done.code.is_empty());
    }

    #[test]
    fn a_missing_cli_is_unavailable() {
        let root = tempfile::TempDir::new().unwrap();
        let facts = shared();
        start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "openai", "seq": 1}),
            |flow| {
                Ok(Runner::Cli(CliLogin {
                    flow,
                    program: root.path().join("absent"),
                    args: Vec::new(),
                    env: Vec::new(),
                    code_wait: Duration::from_secs(1),
                }))
            },
            quiet(),
        )
        .unwrap()
        .join()
        .unwrap();
        let done = fact(&facts).unwrap();
        assert_eq!(
            (done.state, done.reason),
            (SignInState::Unavailable, "cli-missing")
        );
    }

    /// Live: the installed `codex`, found and run the way a tray started by
    /// launchd finds it (`PATH` is only the system's directories), prints a
    /// code this parser reads, and a cancel stops it. OpenAI is asked for a
    /// code that is never approved; nothing is written outside a temporary
    /// `CODEX_HOME`. Run with `cargo test --lib device_login -- --ignored`.
    #[cfg(unix)]
    #[test]
    #[ignore]
    fn live_codex_prints_a_code_this_parser_reads() {
        let home = crate::cache::home_dir().unwrap();
        let launchd_path = std::ffi::OsString::from("/usr/bin:/bin:/usr/sbin:/sbin");
        let program = find_cli("codex", &launchd_path, &home).expect("codex is installed");
        let codex_home = tempfile::TempDir::new().unwrap();
        let facts = shared();
        let handle = start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "openai", "seq": 1}),
            |flow| {
                Ok(Runner::Cli(CliLogin {
                    flow,
                    env: vec![
                        ("CODEX_HOME".into(), codex_home.path().into()),
                        ("PATH".into(), child_path(&program, &launchd_path, &home)),
                        ("NO_COLOR".into(), "1".into()),
                    ],
                    program,
                    args: vec!["login".into(), "--device-auth".into()],
                    code_wait: Duration::from_secs(60),
                }))
            },
            quiet(),
        )
        .unwrap();
        let shown = wait_for(&facts, |f| {
            f.is_some_and(|f| f.state != SignInState::Starting)
        })
        .unwrap();
        assert_eq!(shown.state, SignInState::Waiting, "{shown:?}");
        assert!(is_code(&shown.code), "{shown:?}");
        assert!(Flow::Codex.page_allowed(&shown.url), "{}", shown.url);
        assert!(cancel(&facts, &json!({"seq": 1})));
        handle.join().unwrap();
        assert!(!codex_home.path().join("auth.json").exists());
    }

    #[cfg(unix)]
    fn read_pid(file: &Path) -> i32 {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(pid) = std::fs::read_to_string(file)
                .ok()
                .and_then(|text| text.trim().parse().ok())
            {
                return pid;
            }
            assert!(Instant::now() < until, "the script never wrote {file:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_descendant_that_keeps_writing_cannot_hold_the_flow() {
        // The CLI exits at once and leaves a child writing to its pipes: the
        // last lines are read for a bounded time, and the child goes with it.
        let root = tempfile::TempDir::new().unwrap();
        let pid_file = root.path().join("writer.pid");
        let program = script(
            root.path(),
            &format!(
                "{}\n(while :; do echo tick; sleep 0.05; done) &\necho $! > '{}'\nexit 0",
                printf_codex_prompt(),
                pid_file.display()
            ),
        );
        let facts = shared();
        let started = Instant::now();
        start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "openai", "seq": 1}),
            |flow| Ok(cli(flow, program)),
            quiet(),
        )
        .unwrap()
        .join()
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(fact(&facts).unwrap().state, SignInState::Ok);
        assert!(gone(read_pid(&pid_file)), "the writer outlived its CLI");
        assert!(
            !lock(&RUNNING)
                .pids
                .iter()
                .any(|pid| *pid as i32 == read_pid(&pid_file))
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_descendant_left_behind_goes_with_the_cli() {
        let root = tempfile::TempDir::new().unwrap();
        let pid_file = root.path().join("child.pid");
        let program = script(
            root.path(),
            &format!(
                "sleep 60 >/dev/null 2>&1 &\necho $! > '{}'\nexit 0",
                pid_file.display()
            ),
        );
        let facts = shared();
        start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "openai", "seq": 1}),
            |flow| Ok(cli(flow, program)),
            quiet(),
        )
        .unwrap()
        .join()
        .unwrap();
        assert!(gone(read_pid(&pid_file)), "the CLI's child outlived it");
    }

    #[cfg(unix)]
    #[test]
    fn an_absurd_expiry_is_capped_instead_of_overflowing() {
        let root = tempfile::TempDir::new().unwrap();
        let program = script(
            root.path(),
            "echo 'Opening browser for Kimi device login: https://www.kimi.com/device'\necho 'If the browser did not open, paste the URL above and enter code: WXYZ-1234'\necho 'Code expires in 18446744073709551615s.'\nexec sleep 60",
        );
        let facts = shared();
        let handle = start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "kimi", "seq": 1}),
            |flow| Ok(cli(flow, program)),
            quiet(),
        )
        .unwrap();
        let shown = wait_for(&facts, |f| {
            f.is_some_and(|f| f.state == SignInState::Waiting)
        })
        .unwrap();
        assert!(
            shown.expires_at <= now_ms() + duration_ms(MAX_EXPIRY),
            "{}",
            shown.expires_at
        );
        assert!(cancel(&facts, &json!({"seq": 1})));
        handle.join().expect("the flow did not panic");
    }

    /// A registry of a test's own, so closing it stops nothing elsewhere.
    #[cfg(unix)]
    fn registry() -> &'static Mutex<Running> {
        Box::leak(Box::new(Mutex::new(Running {
            closed: false,
            pids: Vec::new(),
        })))
    }

    #[cfg(unix)]
    #[test]
    fn stop_all_takes_every_cli_along_and_starts_no_other() {
        let root = tempfile::TempDir::new().unwrap();
        let registry = registry();
        let program = script(root.path(), "exec sleep 60");
        let mut command = Command::new(&program);
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let tree = Tree::spawn_in(registry, &mut command).unwrap();
        let pid = tree.child.id() as i32;
        stop_all_in(registry);
        assert!(lock(registry).pids.is_empty());
        // Killed while unreaped: the zombie stays until its flow reaps it.
        drop(tree);
        assert!(gone(pid), "the CLI outlived stop_all");
        // A flow that had not started its CLI yet starts nothing now.
        let marker = root.path().join("started");
        let late = script(root.path(), &format!("touch '{}'", marker.display()));
        assert!(Tree::spawn_in(registry, &mut Command::new(late)).is_err());
        std::thread::sleep(Duration::from_millis(200));
        assert!(!marker.exists());
    }

    #[test]
    fn a_nous_poll_ends_when_the_sheet_says_the_code_is_gone() {
        // Two hours from the portal, a poll interval past the one-hour cap:
        // capped, the poll gives up at once instead of sleeping on.
        let mut server = mockito::Server::new();
        let _device = server
            .mock("POST", "/device")
            .with_status(200)
            .with_body(nous_device_body(7200).replace(r#""interval":1"#, r#""interval":3601"#))
            .create();
        let root = tempfile::TempDir::new().unwrap();
        let store = root.path().join("credentials.json");
        let facts = shared();
        let handle = start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "nous", "seq": 1}),
            |_| Ok(nous(&server, &store)),
            quiet(),
        )
        .unwrap();
        let done = wait_for(&facts, |f| {
            f.is_some_and(|f| !matches!(f.state, SignInState::Starting | SignInState::Waiting))
        })
        .unwrap();
        assert_eq!(done.state, SignInState::Expired);
        handle.join().unwrap();
    }

    #[test]
    fn a_cancel_after_the_approval_is_too_late_and_says_so() {
        let facts = shared();
        waiting(&facts, 3, "https://portal.nousresearch.com/device");
        let job = Job {
            facts: Arc::clone(&facts),
            seq: 3,
        };
        assert!(job.begin_commit());
        assert!(
            !cancel(&facts, &json!({"seq": 3})),
            "a granted login is past cancelling"
        );
        assert!(fact(&facts).is_some_and(|fact| fact.committing));
        // A flow already replaced cannot claim the save.
        let replaced = Job {
            facts: Arc::clone(&facts),
            seq: 2,
        };
        assert!(!replaced.begin_commit());
    }

    #[test]
    fn only_a_codes_shape_is_copied() {
        let facts = shared();
        waiting(&facts, 3, "https://auth.openai.com/codex/device");
        with_facts(&facts, |f| {
            f.sign_in.as_mut().unwrap().code = "NOT A CODE".into()
        });
        assert_eq!(code_to_copy(&facts, &json!({"seq": 3})), None);
    }

    #[test]
    fn a_nous_code_of_another_shape_is_refused() {
        let mut server = mockito::Server::new();
        let _device = server
            .mock("POST", "/device")
            .with_status(200)
            .with_body(nous_device_body(60).replace("NOUS-CODE", "NOT A CODE"))
            .create();
        let root = tempfile::TempDir::new().unwrap();
        let store = root.path().join("credentials.json");
        let facts = shared();
        start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "nous", "seq": 1}),
            |_| Ok(nous(&server, &store)),
            quiet(),
        )
        .unwrap()
        .join()
        .unwrap();
        let done = fact(&facts).unwrap();
        assert_eq!(done.state, SignInState::Failed);
        assert!(done.code.is_empty());
        assert!(!store.exists());
    }

    fn nous_device_body(expires_in: u64) -> String {
        format!(
            r#"{{"device_code":"dev","user_code":"NOUS-CODE","verification_uri":"https://portal.nousresearch.com/device","verification_uri_complete":"https://portal.nousresearch.com/device?user_code=NOUS-CODE","expires_in":{expires_in},"interval":1}}"#
        )
    }

    fn nous(server: &mockito::Server, store: &Path) -> Runner {
        Runner::Nous(NousLogin {
            endpoints: crate::nous::oauth::Endpoints {
                device_code: format!("{}/device", server.url()),
                token: format!("{}/token", server.url()),
            },
            store: crate::nous::credentials::CredentialStore::at(store),
            page_ok: |_| true,
        })
    }

    #[test]
    fn a_nous_login_shows_its_code_then_saves_the_credential() {
        let mut server = mockito::Server::new();
        let _device = server
            .mock("POST", "/device")
            .with_status(200)
            .with_body(nous_device_body(60))
            .create();
        let _token = server
            .mock("POST", "/token")
            .with_status(200)
            .with_body(r#"{"access_token":"access","refresh_token":"refresh","expires_in":3600}"#)
            .create();
        let root = tempfile::TempDir::new().unwrap();
        let store = root.path().join("config").join("credentials.json");
        let facts = shared();
        let (hooks, _, landed) = recording();
        let handle = start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "nous", "seq": 1}),
            |_| Ok(nous(&server, &store)),
            hooks,
        )
        .unwrap();
        let shown = wait_for(&facts, |f| {
            f.is_some_and(|f| f.state != SignInState::Starting)
        })
        .unwrap();
        assert_eq!(shown.code, "NOUS-CODE");
        handle.join().unwrap();
        assert_eq!(fact(&facts).unwrap().state, SignInState::Ok);
        assert_eq!(*landed.lock().unwrap(), vec!["nous".to_string()]);
        let saved = crate::nous::credentials::CredentialStore::at(&store)
            .read()
            .unwrap()
            .and_then(|document| document.nous)
            .unwrap();
        assert_eq!(saved.access_token, "access");
    }

    #[test]
    fn an_expired_nous_code_is_expired_and_saves_nothing() {
        let mut server = mockito::Server::new();
        let _device = server
            .mock("POST", "/device")
            .with_status(200)
            .with_body(nous_device_body(60))
            .create();
        let _token = server
            .mock("POST", "/token")
            .with_status(400)
            .with_body(r#"{"error":"expired_token"}"#)
            .create();
        let root = tempfile::TempDir::new().unwrap();
        let store = root.path().join("credentials.json");
        let facts = shared();
        start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "nous", "seq": 1}),
            |_| Ok(nous(&server, &store)),
            quiet(),
        )
        .unwrap()
        .join()
        .unwrap();
        assert_eq!(fact(&facts).unwrap().state, SignInState::Expired);
        assert!(!store.exists());
    }

    #[test]
    fn a_cancelled_nous_login_saves_nothing() {
        let mut server = mockito::Server::new();
        let _device = server
            .mock("POST", "/device")
            .with_status(200)
            .with_body(nous_device_body(60))
            .create();
        let _token = server
            .mock("POST", "/token")
            .with_status(400)
            .with_body(r#"{"error":"authorization_pending"}"#)
            .create();
        let root = tempfile::TempDir::new().unwrap();
        let store = root.path().join("credentials.json");
        let facts = shared();
        let (hooks, _, landed) = recording();
        let handle = start_with(
            &facts,
            &json!({"cmd": "start-sign-in", "id": "nous", "seq": 7}),
            |_| Ok(nous(&server, &store)),
            hooks,
        )
        .unwrap();
        wait_for(&facts, |f| {
            f.is_some_and(|f| f.state == SignInState::Waiting)
        });
        assert!(cancel(&facts, &json!({"seq": 7})));
        handle.join().unwrap();
        assert_eq!(fact(&facts), None);
        assert!(!store.exists());
        assert!(landed.lock().unwrap().is_empty());
    }
}
