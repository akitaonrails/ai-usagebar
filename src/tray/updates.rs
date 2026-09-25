//! The update worker both tray hosts run: the hourly release check, the snooze
//! file and the install.
//!
//! It lives on the host's worker thread. Every state change lands in the
//! shared facts and is announced so the popover re-renders at once; a verified
//! install hands the new tray's path to the host, which relaunches it and
//! quits. Compiled on every OS so Linux CI runs its tests, like `update_flow`.

#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]

use std::path::PathBuf;

use super::now_ms;
use super::payload::{SharedFacts, UpdateFact, with_facts};
use super::update_flow;
use crate::config::UpdateMode;
use crate::update::{CHECK_INTERVAL, Release, UpdateState};

type Announce = Box<dyn Fn() + Send>;
type Restart = Box<dyn Fn(PathBuf) + Send>;

pub struct Updates {
    /// The HTTP client, or why it could not be built (reported by a manual check).
    client: Result<reqwest::Client, String>,
    facts: SharedFacts,
    /// Test seam: a release URL to ask instead of `latest_release_url()`.
    feed: Option<String>,
    /// The newer release found by the last check, and whether it can be
    /// installed here (assets for this OS/arch, writable install directory).
    pending: Option<(Release, bool)>,
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
        Self {
            client: update_flow::http_client(),
            facts,
            feed: None,
            pending: None,
            state,
            state_path,
            announce,
            restart,
        }
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

    fn persist(&mut self) {
        if let Some(path) = self.state_path.as_deref() {
            let _ = self.state.save_at(path);
        }
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
        self.persist();
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
                let installable = update_flow::installable(&release);
                let snoozed =
                    !manual && self.state.snoozed_version.as_deref() == Some(&release.version);
                let fact = fact_for(&release, "available", String::new(), installable);
                self.pending = Some((release, installable));
                with_facts(&self.facts, |f| {
                    f.update_checked_at = now;
                    f.update = if snoozed { None } else { Some(fact) };
                });
                (self.announce)();
                if self.mode() == UpdateMode::Auto && installable {
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
        let Ok(client) = self.client.clone() else {
            return;
        };
        if !installable {
            return;
        }
        self.set_fact(Some(fact_for(&release, "downloading", String::new(), true)));
        match update_flow::install(&client, &release).await {
            Ok(exe) => {
                self.set_fact(Some(fact_for(&release, "installing", String::new(), true)));
                (self.restart)(exe);
            }
            Err(error) => {
                self.set_fact(Some(fact_for(&release, "failed", error, true)));
            }
        }
    }

    pub fn snooze(&mut self) {
        if let Some((release, _)) = self.pending.as_ref() {
            self.state.snoozed_version = Some(release.version.clone());
            self.persist();
        }
        self.set_fact(None);
    }

    pub async fn set_mode(&mut self, mode: UpdateMode) {
        with_facts(&self.facts, |f| f.updates = mode.as_str().into());
        (self.announce)();
        match mode {
            UpdateMode::Auto if self.pending.as_ref().is_some_and(|(_, ok)| *ok) => {
                self.install().await;
            }
            UpdateMode::Auto | UpdateMode::Notify if self.due() => self.check(false).await,
            _ => {}
        }
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
            state: UpdateState::default(),
            state_path: Some(dir.path().join("update.json")),
            announce: Box::new(|| {}),
            restart: Box::new(|_| {}),
        };
        (updates, facts)
    }

    fn update_fact(facts: &SharedFacts) -> Option<UpdateFact> {
        facts.lock().unwrap().update.clone()
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
