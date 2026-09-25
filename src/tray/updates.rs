//! The update worker both tray hosts run: the hourly release check, the snooze
//! file and the install.
//!
//! It lives on the host's worker thread. Every state change lands in the
//! shared facts and is announced so the popover re-renders at once; an install
//! hands either a new executable path or the Scoop handoff to the host, which
//! quits (and relaunches the executable when given a path). Compiled on every
//! OS so Linux CI runs its tests, like `update_flow`.

#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]

use std::path::{Path, PathBuf};

use super::now_ms;
use super::payload::{SharedFacts, UpdateFact, with_facts};
use super::scoop::ScoopApp;
use super::update_flow;
use crate::config::UpdateMode;
use crate::update::{CHECK_INTERVAL, Release, UpdateState};

type Announce = Box<dyn Fn() + Send>;
type Restart = Box<dyn Fn(Option<PathBuf>) + Send>;
/// Start Scoop's hand-off for the given version (test seam).
type Spawn = Box<dyn Fn(&ScoopApp, &str) -> Result<(), String> + Send>;

pub struct Updates {
    /// The HTTP client, or why it could not be built (reported by a manual check).
    client: Result<reqwest::Client, String>,
    facts: SharedFacts,
    /// Test seam: a release URL to ask instead of `latest_release_url()`.
    feed: Option<String>,
    /// The newer release found by the last check, and whether it can be
    /// installed here (assets for this OS/arch, writable install directory).
    pending: Option<(Release, bool)>,
    scoop: Option<ScoopApp>,
    /// A debug build without a local feed: it may check but never install (test seam).
    dev_build: bool,
    spawn: Spawn,
    state: UpdateState,
    state_path: Option<PathBuf>,
    announce: Announce,
    restart: Restart,
}

impl Updates {
    pub fn new(facts: SharedFacts, announce: Announce, restart: Restart) -> Self {
        let state_path = crate::update::default_state_path().ok();
        let state = state_path
            .as_deref()
            .map(UpdateState::load_at)
            .unwrap_or_default();
        let scoop = if cfg!(windows) {
            crate::tray::scoop::detect().filter(|app| app.can_run(Path::is_file))
        } else {
            None
        };
        let scoop_log = state_path
            .as_deref()
            .and_then(|path| path.parent())
            .map(|dir| dir.join("updates").join("scoop.log"));
        let spawn_log = scoop_log.clone();
        let mut updates = Self {
            client: update_flow::http_client(),
            facts,
            feed: None,
            pending: None,
            scoop,
            dev_build: dev_build(),
            spawn: Box::new(move |app, version| {
                let log = spawn_log
                    .as_deref()
                    .ok_or_else(|| "Scoop update log path is unavailable".to_owned())?;
                crate::tray::scoop::spawn_update(app, std::process::id(), log, version)
            }),
            state,
            state_path,
            announce,
            restart,
        };
        updates.reconcile_handoff();
        updates
    }

    fn mode(&self) -> UpdateMode {
        self.facts
            .lock()
            .ok()
            .and_then(|f| UpdateMode::parse(&f.updates))
            .unwrap_or_default()
    }

    pub fn due(&self) -> bool {
        if self.mode() == UpdateMode::Off {
            return false;
        }
        let elapsed = now_ms().saturating_sub(self.state.last_check_ms);
        elapsed >= CHECK_INTERVAL.as_millis() as i64
    }

    fn persist(&mut self) -> Result<(), String> {
        let path = self
            .state_path
            .as_deref()
            .ok_or_else(|| "update state path is unavailable".to_owned())?;
        self.state.save_at(path).map_err(|error| error.to_string())
    }

    fn scoop_log_path(&self) -> Option<PathBuf> {
        self.state_path
            .as_deref()
            .and_then(Path::parent)
            .map(|dir| dir.join("updates").join("scoop.log"))
    }

    fn reconcile_handoff(&mut self) {
        let Some(version) = self.state.handed_off.clone() else {
            return;
        };
        let running = env!("CARGO_PKG_VERSION");
        if !crate::update::is_newer(running, &version) {
            self.state.handed_off = None;
            let _ = self.persist();
            return;
        }
        let log = self
            .scoop_log_path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "unavailable".to_owned());
        self.set_fact(Some(UpdateFact {
            error: format!("Scoop did not install v{version}; see {log}"),
            installable: self.scoop.is_some() && !self.dev_build,
            state: "failed".into(),
            version,
            ..UpdateFact::default()
        }));
    }

    fn set_fact(&self, fact: Option<UpdateFact>) {
        with_facts(&self.facts, |f| f.update = fact);
        (self.announce)();
    }

    /// Ask for the latest release. A background check (`manual == false`)
    /// honors Off and the snoozed version and stays quiet on failure; a manual
    /// one owes the user an answer either way.
    pub async fn check(&mut self, manual: bool) {
        if !manual && self.mode() == UpdateMode::Off {
            return;
        }
        let now = now_ms();
        let client = match &self.client {
            Ok(client) => client.clone(),
            Err(error) => {
                // The dialog waits for an answer; a check that cannot start is one.
                if manual {
                    let error = error.clone();
                    with_facts(&self.facts, |f| {
                        f.update_checked_at = now;
                        f.update = Some(UpdateFact {
                            error,
                            state: "failed".into(),
                            ..UpdateFact::default()
                        });
                    });
                    (self.announce)();
                }
                return;
            }
        };
        self.state.last_check_ms = now;
        let _ = self.persist();
        if manual {
            // Feedback before the network answers: the dialog reads "Checking…".
            let known = self.pending.as_ref().map(|(release, _)| release);
            self.set_fact(Some(UpdateFact {
                state: "checking".into(),
                url: known.map(|r| r.html_url.clone()).unwrap_or_default(),
                version: known.map(|r| r.version.clone()).unwrap_or_default(),
                ..UpdateFact::default()
            }));
        }
        let current = env!("CARGO_PKG_VERSION");
        let outcome = match &self.feed {
            Some(url) => update_flow::check_at(&client, url, current).await,
            None => update_flow::check(&client, current).await,
        };
        match outcome {
            Ok(Some(release)) => {
                let installable =
                    release_installable(&release, self.scoop.as_ref(), self.dev_build);
                let snoozed =
                    !manual && self.state.snoozed_version.as_deref() == Some(&release.version);
                let version = release.version.clone();
                let fact = fact_for(&release, "available", String::new(), installable);
                self.pending = Some((release, installable));
                with_facts(&self.facts, |f| {
                    f.update_checked_at = now;
                    f.update = if snoozed { None } else { Some(fact) };
                });
                (self.announce)();
                if self.mode() == UpdateMode::Auto
                    && should_auto_install(
                        manual,
                        installable,
                        &version,
                        self.state.handed_off.as_deref(),
                    )
                {
                    self.install().await;
                }
            }
            Ok(None) => {
                self.pending = None;
                with_facts(&self.facts, |f| {
                    f.update_checked_at = now;
                    f.update = None;
                });
                (self.announce)();
            }
            Err(error) => {
                with_facts(&self.facts, |f| {
                    f.update_checked_at = now;
                    if manual {
                        f.update = Some(UpdateFact {
                            error,
                            state: "failed".into(),
                            ..UpdateFact::default()
                        });
                    }
                });
                (self.announce)();
            }
        }
    }

    /// Install what the last check found; with nothing pending (a failed
    /// check's Try Again), check again instead.
    pub async fn install_or_check(&mut self) {
        if self.pending.is_some() {
            self.install().await;
        } else {
            self.check(true).await;
        }
    }

    async fn install(&mut self) {
        let Some((release, installable)) = self.pending.clone() else {
            return;
        };
        if !installable {
            return;
        }
        if let Some(app) = self.scoop.clone() {
            self.set_fact(Some(fact_for(&release, "installing", String::new(), true)));
            self.state.handed_off = Some(release.version.clone());
            if let Err(error) = self.persist() {
                self.state.handed_off = None;
                let _ = self.persist();
                self.set_fact(Some(fact_for(&release, "failed", error, true)));
                return;
            }
            match (self.spawn)(&app, &release.version) {
                Ok(()) => (self.restart)(None),
                Err(error) => {
                    self.state.handed_off = None;
                    let _ = self.persist();
                    self.set_fact(Some(fact_for(&release, "failed", error, true)));
                }
            }
            return;
        }
        let Ok(client) = self.client.clone() else {
            return;
        };
        self.set_fact(Some(fact_for(&release, "downloading", String::new(), true)));
        match update_flow::install(&client, &release).await {
            Ok(exe) => {
                self.set_fact(Some(fact_for(&release, "installing", String::new(), true)));
                (self.restart)(Some(exe));
            }
            Err(error) => {
                self.set_fact(Some(fact_for(&release, "failed", error, true)));
            }
        }
    }

    pub fn snooze(&mut self) {
        if let Some((release, _)) = self.pending.as_ref() {
            self.state.snoozed_version = Some(release.version.clone());
            let _ = self.persist();
        }
        self.set_fact(None);
    }

    pub async fn set_mode(&mut self, mode: UpdateMode) {
        with_facts(&self.facts, |f| f.updates = mode.as_str().into());
        (self.announce)();
        match mode {
            UpdateMode::Auto
                if self.pending.as_ref().is_some_and(|(release, ok)| {
                    should_auto_install(
                        false,
                        *ok,
                        &release.version,
                        self.state.handed_off.as_deref(),
                    )
                }) =>
            {
                self.install().await;
            }
            UpdateMode::Auto | UpdateMode::Notify if self.due() => self.check(false).await,
            _ => {}
        }
    }
}

fn dev_build() -> bool {
    cfg!(debug_assertions) && crate::update::LOCAL_FEED.is_none()
}

fn should_auto_install(
    manual: bool,
    installable: bool,
    version: &str,
    handed_off: Option<&str>,
) -> bool {
    installable && (manual || handed_off != Some(version))
}

fn release_installable(release: &Release, scoop: Option<&ScoopApp>, is_dev_build: bool) -> bool {
    if scoop.is_some() {
        !is_dev_build
    } else {
        update_flow::installable(release)
    }
}

fn fact_for(release: &Release, state: &str, error: String, installable: bool) -> UpdateFact {
    UpdateFact {
        error,
        installable,
        state: state.into(),
        url: release.html_url.clone(),
        version: release.version.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tempfile::TempDir;

    use super::*;
    use crate::tray::payload::HostFacts;

    fn release_json(tag: &str) -> String {
        format!(
            r#"{{"tag_name":"{tag}","prerelease":false,"draft":false,
                "html_url":"https://github.com/akitaonrails/ai-usagebar/releases/tag/{tag}","assets":[]}}"#
        )
    }

    /// An `Updates` asking `feed`, persisting under `dir`, never touching the
    /// real cache or GitHub.
    fn updates_at(dir: &TempDir, feed: String, mode: &str) -> (Updates, SharedFacts) {
        let facts: SharedFacts = Arc::new(Mutex::new(HostFacts::new("0.0.0", false)));
        with_facts(&facts, |f| f.updates = mode.into());
        let updates = Updates {
            client: update_flow::http_client(),
            facts: facts.clone(),
            feed: Some(feed),
            pending: None,
            scoop: None,
            dev_build: false,
            spawn: Box::new(|_, _| Ok(())),
            state: UpdateState::default(),
            state_path: Some(dir.path().join("update.json")),
            announce: Box::new(|| {}),
            restart: Box::new(|_| {}),
        };
        (updates, facts)
    }

    fn scoop_app() -> ScoopApp {
        ScoopApp {
            name: "ai-usagebar".into(),
            root: PathBuf::from("C:/scoop"),
        }
    }

    fn release(version: &str) -> Release {
        Release {
            assets: Vec::new(),
            html_url: format!(
                "https://github.com/akitaonrails/ai-usagebar/releases/tag/v{version}"
            ),
            version: version.into(),
        }
    }

    fn update_fact(facts: &SharedFacts) -> Option<UpdateFact> {
        facts.lock().unwrap().update.clone()
    }

    #[test]
    fn scoop_release_without_assets_is_installable_outside_a_dev_build() {
        let release = release("999.0.0");
        let scoop = scoop_app();
        assert!(release_installable(&release, Some(&scoop), false));
        assert!(!release_installable(&release, Some(&scoop), true));
    }

    #[test]
    fn an_automatic_check_skips_a_version_already_handed_off() {
        assert!(!should_auto_install(
            false,
            true,
            "999.0.0",
            Some("999.0.0")
        ));
        assert!(should_auto_install(true, true, "999.0.0", Some("999.0.0")));
    }

    #[tokio::test]
    async fn a_scoop_install_persists_the_handoff_and_restarts_without_an_exe() {
        let dir = TempDir::new().unwrap();
        let (mut updates, facts) = updates_at(&dir, "http://127.0.0.1:9/unused".into(), "notify");
        updates.scoop = Some(scoop_app());
        updates.pending = Some((release("999.0.0"), true));
        let spawned = Arc::new(Mutex::new(Vec::new()));
        let spawned_by_call = spawned.clone();
        updates.spawn = Box::new(move |app, version| {
            spawned_by_call
                .lock()
                .unwrap()
                .push(format!("{} {version}", app.name));
            Ok(())
        });
        let restarts = Arc::new(Mutex::new(Vec::new()));
        let restarts_by_call = restarts.clone();
        updates.restart = Box::new(move |exe| restarts_by_call.lock().unwrap().push(exe));

        updates.install().await;

        assert_eq!(update_fact(&facts).unwrap().state, "installing");
        assert_eq!(
            UpdateState::load_at(&dir.path().join("update.json"))
                .handed_off
                .as_deref(),
            Some("999.0.0")
        );
        assert_eq!(*spawned.lock().unwrap(), vec!["ai-usagebar 999.0.0"]);
        assert_eq!(*restarts.lock().unwrap(), vec![None]);
    }

    /// Automatic hands a new version to Scoop once. While Scoop's bucket lags, the hourly check
    /// finds the same version again and must not quit the tray again for it; a manual check
    /// (the dialog's Try Again) still hands it off.
    #[tokio::test]
    async fn automatic_never_hands_the_same_version_to_scoop_twice_in_the_background() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/latest")
            .with_body(release_json("v999.0.0"))
            .expect(2)
            .create_async()
            .await;
        let dir = TempDir::new().unwrap();
        let (mut updates, facts) = updates_at(&dir, format!("{}/latest", server.url()), "auto");
        updates.scoop = Some(scoop_app());
        updates.state.handed_off = Some("999.0.0".into());
        let spawned = Arc::new(Mutex::new(0));
        let spawned_by_call = spawned.clone();
        updates.spawn = Box::new(move |_, _| {
            *spawned_by_call.lock().unwrap() += 1;
            Ok(())
        });

        updates.check(false).await;
        assert_eq!(*spawned.lock().unwrap(), 0, "background check skips it");
        let fact = update_fact(&facts).expect("the release is still offered");
        assert_eq!(fact.state, "available");
        assert!(fact.installable);

        updates.check(true).await;
        assert_eq!(*spawned.lock().unwrap(), 1, "a manual check hands it off");
        assert_eq!(update_fact(&facts).unwrap().state, "installing");
    }

    #[tokio::test]
    async fn a_scoop_spawn_error_is_reported_and_clears_the_handoff() {
        let dir = TempDir::new().unwrap();
        let (mut updates, facts) = updates_at(&dir, "http://127.0.0.1:9/unused".into(), "notify");
        updates.scoop = Some(scoop_app());
        updates.pending = Some((release("999.0.0"), true));
        let spawned = Arc::new(Mutex::new(0));
        let spawned_by_call = spawned.clone();
        updates.spawn = Box::new(move |_, _| {
            *spawned_by_call.lock().unwrap() += 1;
            Err("scoop spawn failed".into())
        });
        let restarts = Arc::new(Mutex::new(0));
        let restarts_by_call = restarts.clone();
        updates.restart = Box::new(move |_| *restarts_by_call.lock().unwrap() += 1);

        updates.install().await;

        let fact = update_fact(&facts).unwrap();
        assert_eq!(fact.state, "failed");
        assert_eq!(fact.error, "scoop spawn failed");
        assert_eq!(*spawned.lock().unwrap(), 1);
        assert_eq!(*restarts.lock().unwrap(), 0);
        assert_eq!(
            UpdateState::load_at(&dir.path().join("update.json")).handed_off,
            None
        );
    }

    #[test]
    fn reconciliation_clears_a_delivered_version_and_reports_a_missing_one() {
        let delivered_dir = TempDir::new().unwrap();
        let delivered_path = delivered_dir.path().join("update.json");
        let delivered_state = UpdateState {
            handed_off: Some(env!("CARGO_PKG_VERSION").into()),
            ..UpdateState::default()
        };
        delivered_state.save_at(&delivered_path).unwrap();
        let (mut delivered, delivered_facts) =
            updates_at(&delivered_dir, "unused".into(), "notify");
        delivered.scoop = Some(scoop_app());
        delivered.state = UpdateState::load_at(&delivered_path);
        delivered.reconcile_handoff();
        assert_eq!(delivered.state.handed_off, None);
        assert_eq!(UpdateState::load_at(&delivered_path).handed_off, None);
        assert!(update_fact(&delivered_facts).is_none());

        let missing_dir = TempDir::new().unwrap();
        let missing_path = missing_dir.path().join("update.json");
        let missing_state = UpdateState {
            handed_off: Some("999.0.0".into()),
            ..UpdateState::default()
        };
        missing_state.save_at(&missing_path).unwrap();
        let (mut missing, missing_facts) = updates_at(&missing_dir, "unused".into(), "notify");
        missing.scoop = Some(scoop_app());
        missing.state = UpdateState::load_at(&missing_path);
        missing.reconcile_handoff();
        assert_eq!(missing.state.handed_off.as_deref(), Some("999.0.0"));
        let fact = update_fact(&missing_facts).unwrap();
        assert_eq!(fact.state, "failed");
        assert_eq!(fact.version, "999.0.0");
        assert_eq!(
            fact.error,
            format!(
                "Scoop did not install v999.0.0; see {}",
                missing_dir
                    .path()
                    .join("updates")
                    .join("scoop.log")
                    .display()
            )
        );
    }

    /// A newer release is offered, and a release without assets for this
    /// machine is offered as not installable — the popover then links the
    /// release page instead of showing an Install that cannot work.
    #[tokio::test]
    async fn a_newer_release_without_assets_here_is_available_but_not_installable() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/latest")
            .with_body(release_json("v999.0.0"))
            .create_async()
            .await;
        let dir = TempDir::new().unwrap();
        let (mut updates, facts) = updates_at(&dir, format!("{}/latest", server.url()), "notify");
        updates.check(true).await;
        let fact = update_fact(&facts).expect("a newer release is a fact");
        assert_eq!(fact.state, "available");
        assert_eq!(fact.version, "999.0.0");
        assert!(!fact.installable);
        assert!(facts.lock().unwrap().update_checked_at > 0);
    }

    #[tokio::test]
    async fn the_running_version_clears_the_fact() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/latest")
            .with_body(release_json(&format!("v{}", env!("CARGO_PKG_VERSION"))))
            .create_async()
            .await;
        let dir = TempDir::new().unwrap();
        let (mut updates, facts) = updates_at(&dir, format!("{}/latest", server.url()), "notify");
        with_facts(&facts, |f| {
            f.update = Some(UpdateFact {
                state: "failed".into(),
                ..UpdateFact::default()
            })
        });
        updates.check(true).await;
        assert!(update_fact(&facts).is_none());
    }

    /// A background failure (offline, rate limited) stays quiet; a manual one
    /// is shown.
    #[tokio::test]
    async fn only_a_manual_check_reports_a_failure() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/latest")
            .with_status(503)
            .expect(2)
            .create_async()
            .await;
        let dir = TempDir::new().unwrap();
        let (mut updates, facts) = updates_at(&dir, format!("{}/latest", server.url()), "notify");
        updates.check(false).await;
        assert!(update_fact(&facts).is_none());
        updates.check(true).await;
        let fact = update_fact(&facts).expect("a manual failure is a fact");
        assert_eq!(fact.state, "failed");
        assert!(fact.error.contains("503"), "{}", fact.error);
    }

    /// Snoozing hides that version from the hourly check but not from a
    /// manual one, and the choice survives a restart through the state file.
    #[tokio::test]
    async fn a_snoozed_version_is_hidden_from_background_checks_only() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/latest")
            .with_body(release_json("v999.0.0"))
            .expect(3)
            .create_async()
            .await;
        let dir = TempDir::new().unwrap();
        let (mut updates, facts) = updates_at(&dir, format!("{}/latest", server.url()), "notify");
        updates.check(true).await;
        updates.snooze();
        assert!(update_fact(&facts).is_none());
        let saved = UpdateState::load_at(&dir.path().join("update.json"));
        assert_eq!(saved.snoozed_version.as_deref(), Some("999.0.0"));

        updates.check(false).await;
        assert!(
            update_fact(&facts).is_none(),
            "background check respects the snooze"
        );
        updates.check(true).await;
        assert_eq!(update_fact(&facts).unwrap().state, "available");
    }

    /// A manual check that cannot even build its HTTP client answers the dialog
    /// with that reason instead of leaving it on "Checking…".
    #[tokio::test]
    async fn a_manual_check_without_a_client_reports_why() {
        let dir = TempDir::new().unwrap();
        let (mut updates, facts) = updates_at(&dir, "http://127.0.0.1:9/never".into(), "notify");
        updates.client = Err("no TLS backend".into());
        updates.check(false).await;
        assert!(
            update_fact(&facts).is_none(),
            "a background check stays quiet"
        );
        updates.check(true).await;
        let fact = update_fact(&facts).expect("a manual check answers");
        assert_eq!(fact.state, "failed");
        assert_eq!(fact.error, "no TLS backend");
        assert!(facts.lock().unwrap().update_checked_at > 0);
    }

    #[tokio::test]
    async fn off_skips_the_background_check() {
        let dir = TempDir::new().unwrap();
        let (mut updates, facts) = updates_at(&dir, "http://127.0.0.1:9/never".into(), "off");
        assert!(!updates.due());
        updates.check(false).await;
        assert!(update_fact(&facts).is_none());
        assert_eq!(facts.lock().unwrap().update_checked_at, 0);
    }
}
