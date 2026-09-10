//! The one answer to "which providers exist, how does each authenticate, and
//! is this one switched on and credentialed on this machine".
//!
//! `usage --json` reports only the providers that are *enabled*, which makes
//! the switched-off and the never-credentialed exactly the rows it cannot
//! describe — and those are the rows a "is anything broken?" list exists to
//! show. Filling that gap used to mean a frontend keeping its own provider
//! table, and two of them did: the GNOME extension carried sixteen of the
//! twenty-one providers plus a hand-written TOML reader mirroring
//! `Config::default`, and the macOS menu bar re-derived Claude's, Codex's,
//! Cursor's and Antigravity's credential locations in Swift. Both drifted the
//! moment a provider was added in Rust — Antigravity, Cursor, Kiro, Nous
//! Research and SuperGrok were invisible to the GNOME section for that reason.
//!
//! `ai-usagebar vendors --json` emits this, so a frontend can list every
//! provider, and say what an unusable one is missing, while knowing none of
//! them. It is the `CLAUDE.md` rule that frontend adapters stay thin, applied
//! to the one table that had escaped it.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::vendor::{AuthKind, VendorId};

/// Injected IO, so [`statuses_with`] is a pure function of config plus these
/// answers. Tests pass closures over a fixture and never touch a real `$HOME`,
/// environment variable, or Keychain.
pub struct Probes<'a> {
    /// Whether an environment variable is set to a non-empty value.
    pub env_set: &'a dyn Fn(&str) -> bool,
    /// Whether a path exists.
    pub exists: &'a dyn Fn(&Path) -> bool,
    /// Whether the macOS login Keychain holds Claude Code's OAuth blob. Always
    /// `false` off macOS; a subprocess (`security(1)`) when it is consulted,
    /// which is why it is injected and asked only once Claude's credential
    /// file has already been ruled out.
    pub keychain_has_claude: &'a dyn Fn() -> bool,
}

/// One provider's row in the catalog.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VendorStatus {
    /// Machine id, the same string `usage --json` keys its entries by.
    pub id: &'static str,
    /// Canonical product name, from [`VendorId::display_name`].
    pub name: &'static str,
    pub short_name: &'static str,
    pub kind: AuthKind,
    /// Whether config has this provider switched on.
    pub enabled: bool,
    /// Whether this provider has everything it needs to be fetched. Always
    /// `true` when `needs_credential` is `false`.
    pub configured: bool,
    /// Whether the provider has a credential to be missing at all. Antigravity
    /// has none: there is no file, no key and no login — the binary probes
    /// whichever local product is running — so "not configured" is not a state
    /// it can be in, and a frontend must not offer to fix one.
    pub needs_credential: bool,
    /// Effective environment variable holding this provider's key, honoring an
    /// `api_key_env` override; empty when the provider takes no key.
    pub env: String,
    /// Command that signs this provider in; empty when signing in happens in a
    /// desktop app's own window.
    pub login: &'static str,
}

/// The catalog against the real environment.
pub fn statuses(cfg: &Config) -> Vec<VendorStatus> {
    let probes = Probes {
        env_set: &|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()),
        exists: &|path| path.exists(),
        keychain_has_claude: &keychain_has_claude,
    };
    statuses_with(cfg, &probes)
}

/// One row per [`VendorId::all`], in that canonical order — so a provider
/// added to the enum appears in every frontend with no frontend change, which
/// is the whole point.
pub fn statuses_with(cfg: &Config, probes: &Probes) -> Vec<VendorStatus> {
    VendorId::all()
        .iter()
        .copied()
        .map(|id| {
            // Antigravity is the only provider with nothing to configure.
            let needs_credential = id != VendorId::Antigravity;
            VendorStatus {
                id: id.slug(),
                name: id.display_name(),
                short_name: id.short_name(),
                kind: id.auth_kind(),
                enabled: cfg.is_enabled(id),
                configured: !needs_credential || credential_present(cfg, id, probes),
                needs_credential,
                env: cfg.api_key_env_for(id).to_string(),
                login: id.login_command(),
            }
        })
        .collect()
}

/// Whether this provider's credential is present. Every provider that
/// documents an environment variable is satisfied by it — the OAuth ones
/// included, where it is the headless override — and then by an inline
/// `api_key`, and only then by its own login artifact.
fn credential_present(cfg: &Config, id: VendorId, probes: &Probes) -> bool {
    let env = cfg.api_key_env_for(id);
    if !env.is_empty() && (probes.env_set)(env) {
        return true;
    }
    if cfg.inline_api_key(id).is_some() {
        return true;
    }
    match id {
        // A Keychain-only login is what Claude Code leaves on macOS when no
        // `.credentials.json` was written, so the file alone would report a
        // signed-in user as unconfigured.
        VendorId::Anthropic => {
            any_exists(probes, [crate::anthropic::creds::default_path()])
                || (probes.keychain_has_claude)()
        }
        VendorId::Openai => any_exists(probes, [crate::openai::creds::default_path()]),
        VendorId::Copilot => {
            any_exists(probes, [crate::copilot::credentials::default_hosts_path()])
        }
        VendorId::CommandCode => match crate::commandcode::creds::default_paths() {
            Ok(paths) => paths.iter().any(|path| (probes.exists)(path)),
            Err(_) => false,
        },
        VendorId::NousResearch => {
            (probes.exists)(&crate::nous::credentials::default_credentials_path())
        }
        // Kimi takes a key or the Kimi Code CLI's own OAuth login.
        VendorId::Kimi => any_exists(probes, [kimi_credentials_path(cfg)]),
        // Cursor reads the IDE's state database, falling back to the headless
        // `cursor-agent` CLI's login file — either one means signed in.
        VendorId::Cursor => any_exists(
            probes,
            [
                cfg.cursor
                    .db_path
                    .clone()
                    .map_or_else(crate::cursor::db::default_db_path, Ok),
                cfg.cursor
                    .agent_auth_path
                    .clone()
                    .map_or_else(crate::cursor::db::default_agent_auth_path, Ok),
            ],
        ),
        VendorId::Kiro => any_exists(
            probes,
            [cfg.kiro
                .db_path
                .clone()
                .map_or_else(crate::kiro::db::default_db_path, Ok)],
        ),
        // SuperGrok rides the Grok Build CLI's own login; its executable is the
        // only local artifact, and config pins the trusted path.
        VendorId::Supergrok => (probes.exists)(&cfg.supergrok.grok_binary),
        // Nothing to check: handled by `needs_credential`, never reached.
        VendorId::Antigravity => true,
        // Key-only providers: the environment and inline checks above are the
        // whole answer.
        VendorId::AnthropicApi
        | VendorId::Zai
        | VendorId::Openrouter
        | VendorId::Deepseek
        | VendorId::Kilo
        | VendorId::Novita
        | VendorId::Moonshot
        | VendorId::Grok
        | VendorId::Minimax
        | VendorId::OpenCodeGo
        | VendorId::Ollama => false,
    }
}

fn kimi_credentials_path(cfg: &Config) -> crate::error::Result<PathBuf> {
    match &cfg.kimi.credentials_path {
        Some(path) => Ok(path.clone()),
        None => Ok(crate::kimi::oauth::credentials_path_in(
            &crate::cache::home_dir()?,
        )),
    }
}

/// True when any resolvable path exists. A path that cannot be resolved at all
/// (no `$HOME`) counts as absent rather than as an error: the row still has to
/// render, and "not configured" is the honest thing to draw.
fn any_exists<const N: usize>(probes: &Probes, paths: [crate::error::Result<PathBuf>; N]) -> bool {
    paths
        .iter()
        .filter_map(|path| path.as_ref().ok())
        .any(|path| (probes.exists)(path))
}

#[cfg(target_os = "macos")]
fn keychain_has_claude() -> bool {
    matches!(crate::anthropic::keychain::read_raw(), Ok(Some(_)))
}

#[cfg(not(target_os = "macos"))]
fn keychain_has_claude() -> bool {
    false
}

/// `vendors --json`: the catalog as one JSON document.
pub fn run(json: bool) -> i32 {
    let cfg = match Config::load() {
        Ok(cfg) => cfg,
        Err(error) => {
            eprintln!("vendors: {error}");
            return 1;
        }
    };
    let rows = statuses(&cfg);
    if json {
        match serde_json::to_string(&serde_json::json!({"vendors": rows})) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("vendors: {error}");
                return 1;
            }
        }
        return 0;
    }
    for row in rows {
        let state = if !row.enabled {
            "off"
        } else if row.configured {
            "ready"
        } else {
            "needs credential"
        };
        println!("{:<14} {:<10} {}", row.id, row.kind.as_str(), state);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::settings::KEY_VENDORS;

    /// Every probe answers "no", so a row is configured only because config
    /// says so. Nothing here reads a real `$HOME`, variable or Keychain.
    fn probes<'a>(env: &'a dyn Fn(&str) -> bool, exists: &'a dyn Fn(&Path) -> bool) -> Probes<'a> {
        Probes {
            env_set: env,
            exists,
            keychain_has_claude: &|| false,
        }
    }

    fn bare<'a>() -> Probes<'a> {
        probes(&|_| false, &|_| false)
    }

    fn row(rows: &[VendorStatus], id: &str) -> VendorStatus {
        rows.iter()
            .find(|row| row.id == id)
            .unwrap_or_else(|| panic!("{id} is missing from the catalog"))
            .clone()
    }

    /// The guard this module exists for. A provider added to `VendorId` shows
    /// up here for free; the two frontends that kept their own tables had
    /// silently dropped five of them (Antigravity, Cursor, Kiro, Nous
    /// Research, SuperGrok), in a list whose whole job is to be complete.
    #[test]
    fn every_provider_has_exactly_one_row_in_canonical_order() {
        let rows = statuses_with(&Config::default(), &bare());
        let ids: Vec<&str> = rows.iter().map(|row| row.id).collect();
        let expected: Vec<&str> = VendorId::all().iter().map(|id| id.slug()).collect();
        assert_eq!(ids, expected);
    }

    #[test]
    fn a_key_vendor_is_configured_by_its_environment_variable() {
        let cfg = Config::default();
        let set = |name: &str| name == "ZAI_API_KEY";
        let rows = statuses_with(&cfg, &probes(&set, &|_| false));
        assert!(row(&rows, "zai").configured);
        assert!(!row(&rows, "deepseek").configured);
    }

    #[test]
    fn an_api_key_env_override_is_the_variable_both_reported_and_read() {
        let mut cfg = Config::default();
        cfg.zai.api_key_env = "WORK_ZAI_KEY".to_string();
        let set = |name: &str| name == "WORK_ZAI_KEY";
        let rows = statuses_with(&cfg, &probes(&set, &|_| false));
        let zai = row(&rows, "zai");
        assert_eq!(
            zai.env, "WORK_ZAI_KEY",
            "the row names the effective variable"
        );
        assert!(zai.configured, "and is satisfied by it, not by the default");

        // The default name must no longer count once overridden.
        let stale = |name: &str| name == "ZAI_API_KEY";
        let rows = statuses_with(&cfg, &probes(&stale, &|_| false));
        assert!(!row(&rows, "zai").configured);
    }

    #[test]
    fn an_inline_key_configures_without_the_environment() {
        let mut cfg = Config::default();
        cfg.zai.api_key = Some("sk-inline".to_string());
        let rows = statuses_with(&cfg, &bare());
        assert!(row(&rows, "zai").configured);
    }

    #[test]
    fn an_empty_inline_key_is_not_a_credential() {
        let mut cfg = Config::default();
        cfg.zai.api_key = Some(String::new());
        let rows = statuses_with(&cfg, &bare());
        assert!(!row(&rows, "zai").configured);
    }

    /// Antigravity has no credential of any kind — the binary probes whichever
    /// local product is running — so a frontend must not draw it as missing
    /// one, and must not offer to fix it.
    #[test]
    fn antigravity_has_nothing_to_configure() {
        let rows = statuses_with(&Config::default(), &bare());
        let agy = row(&rows, "antigravity");
        assert!(!agy.needs_credential);
        assert!(agy.configured);
        assert_eq!(agy.env, "");
        assert_eq!(agy.login, "");
    }

    /// Claude Code on macOS may leave the OAuth blob only in the login
    /// Keychain, so the credential file alone would report a signed-in user as
    /// unconfigured.
    #[test]
    fn a_keychain_only_claude_login_counts_as_configured() {
        let cfg = Config::default();
        let with_keychain = Probes {
            env_set: &|_| false,
            exists: &|_| false,
            keychain_has_claude: &|| true,
        };
        assert!(row(&statuses_with(&cfg, &with_keychain), "anthropic").configured);
        assert!(!row(&statuses_with(&cfg, &bare()), "anthropic").configured);
    }

    #[test]
    fn an_oauth_provider_with_no_artifact_names_the_command_that_fixes_it() {
        let rows = statuses_with(&Config::default(), &bare());
        let codex = row(&rows, "openai");
        assert_eq!(codex.kind, AuthKind::Oauth);
        assert!(!codex.configured);
        assert_eq!(codex.login, "codex login");
    }

    /// A provider is only ever fetched when config has it on, and `enabled` is
    /// the one fact `usage --json` cannot report for the rows it omits.
    #[test]
    fn enabled_follows_config_not_the_credential() {
        let mut cfg = Config::default();
        cfg.zai.enabled = false;
        let set = |name: &str| name == "ZAI_API_KEY";
        let zai = row(&statuses_with(&cfg, &probes(&set, &|_| false)), "zai");
        assert!(!zai.enabled, "switched off in config");
        assert!(zai.configured, "but its key is still there");
    }

    /// Auth metadata has to be usable, not merely present: a key provider that
    /// names no variable leaves a frontend with nothing to tell the user.
    #[test]
    fn every_key_provider_names_a_variable_and_every_oauth_one_a_login() {
        let cfg = Config::default();
        for row in statuses_with(&cfg, &bare()) {
            match row.kind {
                AuthKind::ApiKey => assert!(
                    !row.env.is_empty(),
                    "{} authenticates by key but names no variable",
                    row.id
                ),
                AuthKind::Oauth => assert!(
                    !row.login.is_empty(),
                    "{} authenticates by login but names no command",
                    row.id
                ),
                AuthKind::Local => {}
            }
        }
    }

    /// The settings form's credential fields are a *view* over the catalog, so
    /// each one must be a provider the catalog agrees takes a key. This is what
    /// keeps the two from drifting now that the variable name lives in one
    /// place.
    #[test]
    fn the_settings_key_form_covers_only_catalog_key_providers() {
        for kv in KEY_VENDORS {
            assert_eq!(
                kv.id.auth_kind(),
                AuthKind::ApiKey,
                "{} has a key field in Settings but is not a key provider",
                kv.id.slug()
            );
            assert!(
                !kv.id.api_key_env().is_empty(),
                "{} has a key field in Settings but names no variable",
                kv.id.slug()
            );
        }
    }

    #[test]
    fn the_json_document_is_keyed_by_vendors_and_uses_wire_names() {
        let rows = statuses_with(&Config::default(), &bare());
        let text = serde_json::to_string(&serde_json::json!({"vendors": rows})).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        let vendors = parsed["vendors"].as_array().unwrap();
        assert_eq!(vendors.len(), VendorId::all().len());
        assert_eq!(vendors[0]["id"], "anthropic");
        assert_eq!(vendors[0]["kind"], "oauth");
        let agy = vendors
            .iter()
            .find(|v| v["id"] == "antigravity")
            .expect("antigravity is in the report");
        assert_eq!(agy["kind"], "local");
        assert_eq!(agy["needs_credential"], false);
    }
}
