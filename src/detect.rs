//! Local credential detection — the seed for auto-enabling vendors.
//!
//! A fresh install shows the four default vendors and nothing else, even when
//! the machine already carries a Cursor login, a Kiro database, a `gh` OAuth
//! session, or a `KILO_API_KEY`. This module answers "which vendors could
//! fetch right now with what is already on disk?" cheaply enough to run at
//! every frontend start, and turns the answer into a minimal edit of
//! `config.toml`: `enabled = true` for the vendors that have credentials and
//! are not enabled yet. It never writes `false` and never removes anything —
//! the config stays the user's own.
//!
//! Two pieces, mirroring OpenUsage's `hasLocalCredentials` +
//! FirstRunSeeder/NewProviderSeeder:
//!
//! - [`has_local_credentials`] is the per-vendor probe. Files, sqlite, saved
//!   keys, env vars, local port discovery — **never the network**, never a
//!   token refresh, never a cache directory or lock file. It reuses the same
//!   resolvers `build_outcome` reads through, so "detected" means "the fetch
//!   would at least find its credential".
//! - [`DetectState`] remembers which vendors have already been considered, so
//!   a vendor the user deliberately disabled after it was auto-enabled stays
//!   disabled: it is only ever auto-enabled the *first* time it is seen. New
//!   vendors added by an upgrade are not in `known` and get their one chance.
//!   [`plan`] is the pure decision; [`run_once`] is the whole cycle.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{AppError, Result};
use crate::vendor::VendorId;

/// Cheap, local-only probe: files, sqlite, saved keys, env vars. Never the
/// network. `true` means the vendor's fetch would find *a* credential — not
/// that the credential is still valid, which only the wire can tell.
///
/// Exhaustive over [`VendorId`] on purpose: a new vendor fails to compile
/// until it says how it is detected.
pub fn has_local_credentials(vendor: VendorId, config: &Config) -> bool {
    match vendor {
        VendorId::Anthropic => anthropic_present(config),
        VendorId::AnthropicApi => key_present(config, vendor),
        VendorId::Openai => config
            .openai
            .resolve_auth_path(None)
            .is_ok_and(|path| crate::openai::creds::read_from(&path).is_ok()),
        VendorId::Copilot => copilot_present(),
        VendorId::Zai => key_present(config, vendor),
        VendorId::Openrouter => config.openrouter.resolve_api_key(None).is_ok(),
        VendorId::Deepseek => key_present(config, vendor),
        VendorId::Kimi => crate::kimi::resolve_auth(&config.kimi).is_ok(),
        VendorId::Kilo => key_present(config, vendor),
        VendorId::Novita => key_present(config, vendor),
        VendorId::Moonshot => key_present(config, vendor),
        VendorId::Grok => key_present(config, vendor),
        VendorId::Supergrok => crate::supergrok::scope::ScopePaths::with_overrides(
            config.supergrok.auth_path.as_deref(),
            config.supergrok.config_path.as_deref(),
        )
        .is_ok_and(|paths| crate::supergrok::direct::read_billing_key(&paths.auth).is_ok()),
        VendorId::Antigravity => antigravity_present(),
        VendorId::Cursor => cursor_present(config),
        VendorId::Minimax => key_present(config, vendor),
        VendorId::Kiro => {
            let path = match config.kiro.db_path.clone() {
                Some(path) => path,
                None => match crate::kiro::db::default_db_path() {
                    Ok(path) => path,
                    Err(_) => return false,
                },
            };
            crate::kiro::db::read_credentials(&path).is_ok()
        }
        VendorId::NousResearch => {
            // `read_unlocked`, not `read`: the latter takes the store lock,
            // which creates the lock file and a 0700 config dir as a side
            // effect. A probe must leave no trace.
            let store = crate::nous::credentials::CredentialStore::at(
                crate::nous::credentials::default_credentials_path(),
            );
            matches!(store.read_unlocked(), Ok(Some(_)))
        }
        VendorId::OpenCodeGo => key_present(config, vendor),
        VendorId::CommandCode => {
            crate::commandcode::creds::resolve(config.commandcode.auth_paths.as_deref()).is_ok()
        }
        VendorId::Ollama => key_present(config, vendor),
    }
}

/// A key vendor: the configured env var (`api_key_env`, defaulting to
/// `VendorId::api_key_env`) or the inline `api_key`, exactly as the fetch
/// resolves them. `Config::api_key_env_for` / `inline_api_key` are the shared
/// per-vendor lookup, so a new key vendor needs no arm of its own here.
fn key_present(config: &Config, vendor: VendorId) -> bool {
    crate::config::optional_api_key(
        config.api_key_env_for(vendor),
        config.inline_api_key(vendor),
    )
    .is_some()
}

/// The default Claude account exactly as the fetch resolves it: an explicit
/// `credentials_path` is a strict file read; the platform default adds the
/// macOS Keychain fallback inside `creds::resolve` (gated there, not here).
fn anthropic_present(config: &Config) -> bool {
    use crate::anthropic::creds::{CredsTarget, default_path, resolve};
    let target = match config.anthropic.credentials_path.clone() {
        Some(path) => CredsTarget::Explicit(path),
        None => match default_path() {
            Ok(path) => CredsTarget::Default(path),
            Err(_) => return false,
        },
    };
    resolve(&target).is_ok()
}

/// GitHub Copilot detection is a **new** heuristic, deliberately different
/// from the fetch: the fetch spawns `gh auth token` and lets the GitHub CLI
/// decide, but a probe that runs at every frontend start must not fork a
/// subprocess per vendor. So this asks the two questions `gh auth token`
/// would answer from: an explicit `GITHUB_COPILOT_TOKEN`, or the `hosts.yml`
/// that `gh auth login` has written, at the path `gh` itself would read
/// (`copilot::credentials::default_hosts_path`). A present `hosts.yml` is
/// treated as a login; if the session inside it has been revoked, the fetch
/// reports that, as it would for any stale credential.
fn copilot_present() -> bool {
    if std::env::var_os("GITHUB_COPILOT_TOKEN").is_some_and(|value| !value.is_empty()) {
        return true;
    }
    crate::copilot::credentials::default_hosts_path()
        .is_ok_and(|path| copilot_hosts_present_at(&path))
}

/// A `hosts.yml` counts when it is a regular, non-empty file: `gh auth logout`
/// of the last host leaves an empty document behind, which is not a login.
pub(crate) fn copilot_hosts_present_at(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
}

/// Antigravity has no API key: it is "present" when a product is running and
/// reachable, which is a local port question. The env override counts on its
/// own, as it does for the fetch's candidate list. Cheapest check first.
fn antigravity_present() -> bool {
    if std::env::var_os("ANTIGRAVITY_LS_ADDRESS").is_some_and(|value| !value.is_empty()) {
        return true;
    }
    !crate::antigravity::fetch::discover_ls_ports().is_empty()
}

fn cursor_present(config: &Config) -> bool {
    let db_path = match config.cursor.db_path.clone() {
        Some(path) => path,
        None => match crate::cursor::db::default_db_path() {
            Ok(path) => path,
            Err(_) => return false,
        },
    };
    let agent_auth_path = match config.cursor.agent_auth_path.clone() {
        Some(path) => path,
        None => match crate::cursor::db::default_agent_auth_path() {
            Ok(path) => path,
            Err(_) => return false,
        },
    };
    crate::cursor::db::resolve_access_token(&db_path, &agent_auth_path).is_ok()
}

/// Which vendors detection has already had its one look at. Persisted as
/// JSON next to the vendor caches; a vendor in `known` is never auto-enabled
/// again, so a user's later `enabled = false` sticks.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectState {
    #[serde(default)]
    pub known: Vec<VendorId>,
}

impl DetectState {
    /// Missing or unreadable state means "nothing has been considered yet",
    /// which only ever widens the config — the safe direction for a corrupt
    /// sidecar. A slug the current build doesn't know parses as corrupt too.
    pub fn load_at(path: &Path) -> DetectState {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Atomic write (tempfile + rename), creating the parent directory.
    pub fn save_at(&self, path: &Path) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(self)?;
        crate::cache::atomic_write(path, &bytes)
    }
}

/// `<cache dir>/ai-usagebar/detect.json` — beside the per-vendor caches,
/// because it is derived state that can be deleted to re-run detection.
pub fn default_state_path() -> Result<PathBuf> {
    Ok(crate::cache::xdg_cache_dir()?
        .join("ai-usagebar")
        .join("detect.json"))
}

/// What one detection pass decided.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DetectPlan {
    /// Vendors to flip to `enabled = true`, in `all` order.
    pub enable: Vec<VendorId>,
    /// The next `DetectState::known`: the old set plus everything in `all`.
    pub known: Vec<VendorId>,
    /// How many candidates this pass considered: every vendor in `all` when
    /// forced, otherwise only the ones not yet in `state.known`.
    pub probed: usize,
}

/// What one full [`run_once_with`] cycle did — the CLI's report and the
/// serialized shape of `detect --json` (vendors as slugs, no secrets).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct DetectReport {
    /// Vendors flipped to `enabled = true` in the config this run.
    pub enabled: Vec<VendorId>,
    /// The persisted `DetectState::known` after this run.
    pub known: Vec<VendorId>,
    /// How many vendors were candidates this run (see [`DetectPlan::probed`]).
    pub probed: usize,
}

/// The pure decision. Candidates are `all` minus `state.known`, or every
/// vendor in `all` when `force`. A candidate is enabled when `probe` says it
/// has credentials and the config doesn't already enable it. `known` becomes
/// the union of the old set and `all`, in [`VendorId::all`] order, deduped —
/// so a vendor is considered once per install, and once more per `force`.
pub fn plan(
    config: &Config,
    state: &DetectState,
    all: &[VendorId],
    force: bool,
    probe: impl Fn(VendorId) -> bool,
) -> DetectPlan {
    let candidates: Vec<VendorId> = all
        .iter()
        .copied()
        .filter(|vendor| force || !state.known.contains(vendor))
        .collect();
    let probed = candidates.len();
    let enable = candidates
        .into_iter()
        .filter(|vendor| !config.is_enabled(*vendor))
        .filter(|vendor| probe(*vendor))
        .collect();
    let known = VendorId::all()
        .iter()
        .copied()
        .filter(|vendor| state.known.contains(vendor) || all.contains(vendor))
        .collect();
    DetectPlan {
        enable,
        known,
        probed,
    }
}

/// One full detection cycle: load the config (`config_path`, or the resolved
/// default), load the state at `state_path`, plan with
/// [`has_local_credentials`], write the enables into the config, save the
/// state, and return what was enabled.
///
/// Best-effort by design: every probe runs under `catch_unwind`, so a vendor
/// resolver that panics on an unexpected local file counts as "not present"
/// rather than taking the host process down.
pub fn run_once(
    config_path: Option<&Path>,
    state_path: &Path,
    force: bool,
) -> Result<Vec<VendorId>> {
    run_once_report(config_path, state_path, force).map(|report| report.enabled)
}

/// [`run_once`] keeping the whole [`DetectReport`] — what the `detect`
/// subcommand prints. Same real probe, same `catch_unwind` guard.
pub fn run_once_report(
    config_path: Option<&Path>,
    state_path: &Path,
    force: bool,
) -> Result<DetectReport> {
    run_once_with(config_path, state_path, force, |vendor, config| {
        catch_unwind(AssertUnwindSafe(|| has_local_credentials(vendor, config))).unwrap_or(false)
    })
}

/// `ai-usagebar detect [--all] [--json]`: one-shot local credential detection
/// as a command, so any frontend (or the user) can run it at startup.
/// Uses the real config and state paths — tests go through
/// [`run_once_with`] and [`format_report`] instead.
pub fn run_cli(all: bool, json: bool) -> i32 {
    let report =
        default_state_path().and_then(|state_path| run_once_report(None, &state_path, all));
    match report {
        Ok(report) if json => match serde_json::to_string(&report) {
            Ok(text) => {
                println!("{text}");
                0
            }
            Err(error) => {
                eprintln!("ai-usagebar detect: {error}");
                1
            }
        },
        Ok(report) => {
            println!(
                "{}",
                format_report(&report, &crate::config::config_path_hint())
            );
            0
        }
        Err(error) => {
            eprintln!("ai-usagebar detect: {}", error.user_message());
            1
        }
    }
}

/// Human-readable `detect` output. `config_hint` is where the enables were
/// written, shown only when something was enabled.
pub fn format_report(report: &DetectReport, config_hint: &str) -> String {
    if report.enabled.is_empty() {
        let noun = if report.probed == 1 {
            "vendor"
        } else {
            "vendors"
        };
        return format!("Nothing new detected ({} {noun} checked)", report.probed);
    }
    let names: Vec<&str> = report
        .enabled
        .iter()
        .map(|vendor| vendor.display_name())
        .collect();
    format!("Enabled: {}\nWritten to {config_hint}", names.join(", "))
}

/// [`run_once_report`] with the probe injected — the test seam, so the
/// cycle's config write and state bookkeeping can be exercised without a
/// probe that reads this machine's real credential files.
pub fn run_once_with(
    config_path: Option<&Path>,
    state_path: &Path,
    force: bool,
    probe: impl Fn(VendorId, &Config) -> bool,
) -> Result<DetectReport> {
    let resolved = match config_path {
        Some(path) => Some(path.to_path_buf()),
        None => crate::config::resolved_path(),
    };
    let config = match &resolved {
        Some(path) => Config::load_from(path)?,
        None => Config::default(),
    };
    let state = DetectState::load_at(state_path);
    let plan = plan(&config, &state, VendorId::all(), force, |vendor| {
        probe(vendor, &config)
    });
    // What actually got written, which is not always what was planned: a
    // vendor the user explicitly set to `enabled = false` is left alone by
    // `enable_vendors_in`, so it must not be reported as enabled either.
    let enabled = if plan.enable.is_empty() {
        Vec::new()
    } else {
        let path = resolved.ok_or_else(|| {
            AppError::Other("could not resolve the config.toml path to enable vendors in".into())
        })?;
        crate::config::enable_vendors_in(&path, &plan.enable)?
    };
    DetectState {
        known: plan.known.clone(),
    }
    .save_at(state_path)?;
    Ok(DetectReport {
        enabled,
        known: plan.known,
        probed: plan.probed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn probe_in(present: &[VendorId]) -> impl Fn(VendorId) -> bool + '_ {
        move |vendor| present.contains(&vendor)
    }

    #[test]
    fn state_round_trips_through_json_by_slug() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("detect.json");
        let state = DetectState {
            known: vec![
                VendorId::Cursor,
                VendorId::OpenCodeGo,
                VendorId::NousResearch,
            ],
        };

        state.save_at(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"opencode-go\""), "{text}");
        assert!(text.contains("\"nous\""), "{text}");
        assert_eq!(DetectState::load_at(&path), state);
    }

    #[test]
    fn missing_or_corrupt_state_is_the_default() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            DetectState::load_at(&dir.path().join("absent.json")),
            DetectState::default()
        );

        let corrupt = dir.path().join("corrupt.json");
        std::fs::write(&corrupt, "{\"known\": [\"not-a-vendor\"").unwrap();
        assert_eq!(DetectState::load_at(&corrupt), DetectState::default());

        let unknown_slug = dir.path().join("unknown.json");
        std::fs::write(&unknown_slug, "{\"known\": [\"not-a-vendor\"]}").unwrap();
        assert_eq!(DetectState::load_at(&unknown_slug), DetectState::default());
    }

    #[test]
    fn plan_enables_only_unknown_probed_vendors_that_are_off() {
        let config = Config::default(); // anthropic/openai/zai/openrouter on
        let state = DetectState {
            known: vec![VendorId::Grok],
        };
        let all = [
            VendorId::Anthropic, // enabled already → never listed
            VendorId::Grok,      // known → skipped
            VendorId::Cursor,    // present, off, new → enabled
            VendorId::Kiro,      // absent → not enabled
        ];
        let present = [VendorId::Anthropic, VendorId::Grok, VendorId::Cursor];

        let plan = plan(&config, &state, &all, false, probe_in(&present));

        assert_eq!(plan.enable, vec![VendorId::Cursor]);
    }

    #[test]
    fn force_reconsiders_known_vendors_but_never_enabled_ones() {
        let config = Config::default();
        let state = DetectState {
            known: vec![VendorId::Grok, VendorId::Zai],
        };
        let all = [VendorId::Zai, VendorId::Grok];
        let present = [VendorId::Zai, VendorId::Grok];

        let plan = plan(&config, &state, &all, true, probe_in(&present));

        assert_eq!(plan.enable, vec![VendorId::Grok]);
    }

    #[test]
    fn plan_orders_enable_by_the_candidate_list() {
        let config = Config::default();
        let all = [VendorId::Kiro, VendorId::Cursor, VendorId::Grok];
        let present = [VendorId::Grok, VendorId::Cursor, VendorId::Kiro];

        let plan = plan(
            &config,
            &DetectState::default(),
            &all,
            false,
            probe_in(&present),
        );

        assert_eq!(
            plan.enable,
            vec![VendorId::Kiro, VendorId::Cursor, VendorId::Grok]
        );
    }

    #[test]
    fn known_becomes_the_union_in_canonical_order_without_duplicates() {
        let config = Config::default();
        let state = DetectState {
            known: vec![VendorId::Grok, VendorId::Cursor],
        };
        let all = [VendorId::Cursor, VendorId::Anthropic, VendorId::Cursor];

        let plan = plan(&config, &state, &all, false, |_| false);

        assert_eq!(
            plan.known,
            vec![VendorId::Anthropic, VendorId::Grok, VendorId::Cursor]
        );
        assert!(plan.enable.is_empty());
    }

    #[test]
    fn probe_is_not_consulted_for_skipped_vendors() {
        let config = Config::default();
        let state = DetectState {
            known: vec![VendorId::Grok],
        };
        let all = [VendorId::Grok, VendorId::Anthropic];

        let plan = plan(&config, &state, &all, false, |vendor| {
            panic!("probe called for {}", vendor.slug())
        });

        assert!(plan.enable.is_empty());
    }

    #[test]
    fn copilot_hosts_file_must_be_a_non_empty_regular_file() {
        let dir = TempDir::new().unwrap();
        let hosts = dir.path().join("hosts.yml");
        assert!(!copilot_hosts_present_at(&hosts));

        std::fs::write(&hosts, "").unwrap();
        assert!(!copilot_hosts_present_at(&hosts));

        std::fs::write(&hosts, "github.com:\n    user: octocat\n").unwrap();
        assert!(copilot_hosts_present_at(&hosts));

        assert!(!copilot_hosts_present_at(dir.path()));
    }

    /// The whole cycle against a temp config and state, with the probe faked:
    /// enables land in the config with the user's text intact, the state
    /// records every vendor, and a second run has nothing left to do.
    #[test]
    fn run_once_writes_enables_into_the_config_and_marks_everything_known() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let state_path = dir.path().join("detect.json");
        std::fs::write(
            &config_path,
            "# mine
[zai]
enabled = false
",
        )
        .unwrap();
        let present = [VendorId::Zai, VendorId::Cursor, VendorId::Anthropic];
        let probe = |vendor: VendorId, _: &Config| present.contains(&vendor);

        let report = run_once_with(Some(&config_path), &state_path, false, probe).unwrap();

        // Z.AI is detectable and was probed, but the config says `enabled =
        // false`. That is the user's answer and it outranks detection, so it is
        // neither written nor reported as enabled.
        assert_eq!(report.enabled, vec![VendorId::Cursor]);
        assert_eq!(report.known, VendorId::all());
        assert_eq!(report.probed, VendorId::all().len());
        let after = Config::load_from(&config_path).unwrap();
        assert!(!after.is_enabled(VendorId::Zai), "an opt-out must survive");
        assert!(after.is_enabled(VendorId::Cursor));
        let text = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            text.starts_with(
                "# mine
"
            ),
            "{text}"
        );
        assert_eq!(DetectState::load_at(&state_path).known, VendorId::all());

        let again = run_once_with(Some(&config_path), &state_path, false, probe).unwrap();
        assert!(again.enabled.is_empty(), "{again:?}");
        assert_eq!(again.probed, 0, "everything is known: nothing to check");

        // A user who turns Cursor back off is not overridden: it is known.
        std::fs::write(
            &config_path,
            "[cursor]
enabled = false
",
        )
        .unwrap();
        let third = run_once_with(Some(&config_path), &state_path, false, probe).unwrap();
        assert!(third.enabled.is_empty(), "{third:?}");
        assert!(
            !Config::load_from(&config_path)
                .unwrap()
                .is_enabled(VendorId::Cursor)
        );

        // `force` re-probes every vendor, but it still cannot overrule an
        // explicit `enabled = false`. That makes `detect --all` safe to run at
        // any time: it can add providers, never silently undo a decision. A
        // user who wants Cursor back turns it on in Settings or in the file.
        let forced = run_once_with(Some(&config_path), &state_path, true, probe).unwrap();
        assert!(forced.enabled.is_empty(), "{forced:?}");
        assert_eq!(forced.probed, VendorId::all().len());
        assert!(
            !Config::load_from(&config_path)
                .unwrap()
                .is_enabled(VendorId::Cursor)
        );
    }

    #[test]
    fn plan_counts_candidates_not_enables() {
        let config = Config::default();
        let state = DetectState {
            known: vec![VendorId::Grok],
        };
        let all = [VendorId::Anthropic, VendorId::Grok, VendorId::Cursor];

        let unforced = plan(&config, &state, &all, false, |_| false);
        assert_eq!(unforced.probed, 2, "Grok is known and skipped");

        let forced = plan(&config, &state, &all, true, |_| false);
        assert_eq!(forced.probed, 3);
    }

    #[test]
    fn format_report_lists_display_names_and_where_they_were_written() {
        let report = DetectReport {
            enabled: vec![VendorId::Cursor, VendorId::Kiro],
            known: VendorId::all().to_vec(),
            probed: 3,
        };

        let text = format_report(&report, "/home/u/.config/ai-usagebar/config.toml");

        assert_eq!(
            text,
            "Enabled: Cursor, Kiro\nWritten to /home/u/.config/ai-usagebar/config.toml"
        );
    }

    #[test]
    fn format_report_says_how_many_were_checked_when_nothing_changed() {
        let none = DetectReport {
            enabled: vec![],
            known: VendorId::all().to_vec(),
            probed: 3,
        };
        assert_eq!(
            format_report(&none, "unused"),
            "Nothing new detected (3 vendors checked)"
        );

        let one = DetectReport {
            probed: 1,
            ..none.clone()
        };
        assert_eq!(
            format_report(&one, "unused"),
            "Nothing new detected (1 vendor checked)"
        );
    }

    /// The `--json` contract: slugs, three fields, no paths and no secrets.
    #[test]
    fn report_serializes_slugs_and_the_probed_count() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let state_path = dir.path().join("detect.json");
        std::fs::write(&config_path, "").unwrap();
        DetectState {
            known: vec![VendorId::Anthropic, VendorId::Grok],
        }
        .save_at(&state_path)
        .unwrap();
        let probe = |vendor: VendorId, _: &Config| vendor == VendorId::Cursor;

        let report = run_once_with(Some(&config_path), &state_path, false, probe).unwrap();
        let json: serde_json::Value = serde_json::to_value(&report).unwrap();

        assert_eq!(json["enabled"], serde_json::json!(["cursor"]));
        assert_eq!(
            json["probed"],
            serde_json::json!(VendorId::all().len() - 2),
            "the two known vendors were not candidates"
        );
        let known = json["known"].as_array().unwrap();
        assert_eq!(known.len(), VendorId::all().len());
        assert_eq!(known[0], serde_json::json!("anthropic"));
        assert_eq!(json.as_object().unwrap().len(), 3, "{json}");
    }

    #[test]
    fn a_panicking_probe_counts_as_absent_and_still_saves_state() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let state_path = dir.path().join("detect.json");
        let probe = |vendor: VendorId, _: &Config| {
            catch_unwind(AssertUnwindSafe(|| {
                if vendor == VendorId::Kiro {
                    panic!("boom");
                }
                vendor == VendorId::Grok
            }))
            .unwrap_or(false)
        };

        let enabled = run_once_with(Some(&config_path), &state_path, false, probe).unwrap();

        assert_eq!(enabled.enabled, vec![VendorId::Grok]);
        assert_eq!(DetectState::load_at(&state_path).known, VendorId::all());
    }
}
