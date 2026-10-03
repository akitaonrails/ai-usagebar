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
    /// Whether the macOS login Keychain holds Claude Code's OAuth blob for a
    /// specific account's `CLAUDE_CONFIG_DIR`. Always `false` off macOS;
    /// consulted only once the account's credentials file is ruled out.
    pub keychain_has_claude_for: &'a dyn Fn(&Path) -> bool,
    /// Whether one of Command Code's auth files holds a live credential. Its
    /// search list includes pi's shared keystore, which exists whenever the
    /// user signed pi into any provider, so existence alone cannot mean
    /// "signed in" the way the provider-owned files above can.
    pub commandcode_signed_in: &'a dyn Fn(&[PathBuf]) -> bool,
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
        keychain_has_claude_for: &keychain_has_claude_for,
        commandcode_signed_in: &|paths| {
            paths
                .iter()
                .any(|path| crate::commandcode::creds::read_from(path).is_some())
        },
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
    if cfg.api_key_accounts(id).is_some_and(|accounts| {
        accounts.iter().any(|account| {
            account
                .api_key_env
                .as_deref()
                .filter(|name| !name.is_empty())
                .is_some_and(|name| (probes.env_set)(name))
                || account.api_key.as_deref().is_some_and(|k| !k.is_empty())
        })
    }) {
        return true;
    }
    match id {
        // A Keychain-only login is what Claude Code leaves on macOS when no
        // `.credentials.json` was written, so the file alone would report a
        // signed-in user as unconfigured.
        VendorId::Anthropic => {
            let default_or_explicit = any_exists(probes, [anthropic_credentials_path(cfg)]);
            let keychain =
                cfg.anthropic.credentials_path.is_none() && (probes.keychain_has_claude)();
            default_or_explicit
                || keychain
                || cfg.anthropic.all_accounts().iter().any(|account| {
                    (probes.exists)(&account.credentials_path)
                        || (probes.keychain_has_claude_for)(&account.config_dir())
                })
        }
        VendorId::Openai => {
            any_exists(probes, [cfg.openai.resolve_auth_path(None)])
                || cfg
                    .openai
                    .accounts
                    .iter()
                    .any(|account| (probes.exists)(&account.codex_auth_path))
        }
        VendorId::Copilot => {
            any_exists(probes, [crate::copilot::credentials::default_hosts_path()])
        }
        VendorId::CommandCode => {
            match crate::commandcode::creds::effective_paths(cfg.commandcode.auth_paths.as_deref())
            {
                Ok(paths) => (probes.commandcode_signed_in)(&paths),
                Err(_) => false,
            }
        }
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
        // The Grok Bot desktop app's own credential file is the login.
        VendorId::Grokbot => any_exists(probes, [crate::grokbot::secrets_path(&cfg.grokbot)]),
        // The `bl` CLI's own console-login file is the login.
        VendorId::ModelStudio => {
            any_exists(probes, [crate::modelstudio::config_path(&cfg.modelstudio)])
        }
        // Nothing to check: handled by `needs_credential`, never reached.
        VendorId::Antigravity => true,
        // Key-only providers: the environment and inline checks above are the
        // whole answer.
        VendorId::AnthropicApi
        | VendorId::Zai
        | VendorId::Openrouter
        | VendorId::Deepseek
        | VendorId::Deepinfra
        | VendorId::Kilo
        | VendorId::Novita
        | VendorId::Moonshot
        | VendorId::Grok
        | VendorId::Minimax
        | VendorId::OpenCodeGo
        | VendorId::Ollama
        | VendorId::OrcaRouter => false,
    }
}

fn anthropic_credentials_path(cfg: &Config) -> crate::error::Result<PathBuf> {
    match &cfg.anthropic.credentials_path {
        Some(path) => Ok(path.clone()),
        None => crate::anthropic::creds::default_path(),
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

#[cfg(target_os = "macos")]
fn keychain_has_claude_for(config_dir: &Path) -> bool {
    matches!(
        crate::anthropic::keychain::read_raw_for(config_dir),
        Ok(Some(_))
    )
}

#[cfg(not(target_os = "macos"))]
fn keychain_has_claude_for(_config_dir: &Path) -> bool {
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
    fn probes<'a>(
        env: &'a dyn Fn(&str) -> bool,
        exists: &'a dyn Fn(&Path) -> bool,
        commandcode_signed_in: &'a dyn Fn(&[PathBuf]) -> bool,
    ) -> Probes<'a> {
        Probes {
            env_set: env,
            exists,
            keychain_has_claude: &|| false,
            keychain_has_claude_for: &|_| false,
            commandcode_signed_in,
        }
    }

    fn bare<'a>() -> Probes<'a> {
        probes(&|_| false, &|_| false, &|_| false)
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
        let rows = statuses_with(&cfg, &probes(&set, &|_| false, &|_| false));
        assert!(row(&rows, "zai").configured);
        assert!(!row(&rows, "deepseek").configured);
    }

    #[test]
    fn an_api_key_env_override_is_the_variable_both_reported_and_read() {
        let mut cfg = Config::default();
        cfg.zai.api_key_env = "WORK_ZAI_KEY".to_string();
        let set = |name: &str| name == "WORK_ZAI_KEY";
        let rows = statuses_with(&cfg, &probes(&set, &|_| false, &|_| false));
        let zai = row(&rows, "zai");
        assert_eq!(
            zai.env, "WORK_ZAI_KEY",
            "the row names the effective variable"
        );
        assert!(zai.configured, "and is satisfied by it, not by the default");

        // The default name must no longer count once overridden.
        let stale = |name: &str| name == "ZAI_API_KEY";
        let rows = statuses_with(&cfg, &probes(&stale, &|_| false, &|_| false));
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

    #[test]
    fn a_named_api_key_account_counts_as_configured() {
        let mut cfg = Config::default();
        cfg.openrouter.accounts.push(crate::config::ApiKeyAccount {
            label: "work".into(),
            api_key_env: None,
            api_key: Some("sk-named".into()),
            management_api_key_env: None,
        });
        let rows = statuses_with(&cfg, &bare());
        assert!(row(&rows, "openrouter").configured);
        assert!(!row(&rows, "deepseek").configured);
    }

    #[test]
    fn a_named_api_key_account_with_env_counts_as_configured() {
        let mut cfg = Config::default();
        cfg.deepseek.accounts.push(crate::config::ApiKeyAccount {
            label: "work".into(),
            api_key_env: Some("DEEPSEEK_WORK_API_KEY".into()),
            api_key: None,
            management_api_key_env: None,
        });
        let set = |name: &str| name == "DEEPSEEK_WORK_API_KEY";
        let rows = statuses_with(&cfg, &probes(&set, &|_| false, &|_| false));
        assert!(row(&rows, "deepseek").configured);
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
            keychain_has_claude_for: &|_| false,
            commandcode_signed_in: &|_| false,
        };
        assert!(row(&statuses_with(&cfg, &with_keychain), "anthropic").configured);
        assert!(!row(&statuses_with(&cfg, &bare()), "anthropic").configured);
    }

    #[test]
    fn an_anthropic_credentials_path_override_counts_as_configured() {
        let mut cfg = Config::default();
        let custom = PathBuf::from("/custom/anthropic/.credentials.json");
        cfg.anthropic.credentials_path = Some(custom.clone());
        let exists = |path: &Path| path == custom;
        let rows = statuses_with(&cfg, &probes(&|_| false, &exists, &|_| false));
        assert!(row(&rows, "anthropic").configured);
    }

    #[test]
    fn an_anthropic_named_account_counts_as_configured() {
        let mut cfg = Config::default();
        let custom = PathBuf::from("/accounts/work/.credentials.json");
        cfg.anthropic
            .accounts
            .push(crate::config::AnthropicAccount {
                label: "work".into(),
                credentials_path: custom.clone(),
            });
        let exists = |path: &Path| path == custom;
        let rows = statuses_with(&cfg, &probes(&|_| false, &exists, &|_| false));
        assert!(row(&rows, "anthropic").configured);
    }

    #[test]
    fn a_keychain_only_anthropic_named_account_counts_as_configured() {
        let mut cfg = Config::default();
        let custom = PathBuf::from("/accounts/work/.credentials.json");
        cfg.anthropic
            .accounts
            .push(crate::config::AnthropicAccount {
                label: "work".into(),
                credentials_path: custom,
            });
        let with_named_keychain = Probes {
            env_set: &|_| false,
            exists: &|_| false,
            keychain_has_claude: &|| false,
            keychain_has_claude_for: &|dir| dir == Path::new("/accounts/work"),
            commandcode_signed_in: &|_| false,
        };
        assert!(row(&statuses_with(&cfg, &with_named_keychain), "anthropic").configured);
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

    #[test]
    fn an_openai_codex_auth_path_override_counts_as_configured() {
        let mut cfg = Config::default();
        let custom = PathBuf::from("/custom/openai/auth.json");
        cfg.openai.codex_auth_path = Some(custom.clone());
        let exists = |path: &Path| path == custom;
        let rows = statuses_with(&cfg, &probes(&|_| false, &exists, &|_| false));
        assert!(row(&rows, "openai").configured);
    }

    #[test]
    fn an_openai_named_account_counts_as_configured() {
        let mut cfg = Config::default();
        let custom = PathBuf::from("/accounts/work/auth.json");
        cfg.openai.accounts.push(crate::config::OpenAiAccount {
            label: "work".into(),
            codex_auth_path: custom.clone(),
        });
        let exists = |path: &Path| path == custom;
        let rows = statuses_with(&cfg, &probes(&|_| false, &exists, &|_| false));
        assert!(row(&rows, "openai").configured);
    }

    /// A provider is only ever fetched when config has it on, and `enabled` is
    /// the one fact `usage --json` cannot report for the rows it omits.
    #[test]
    fn enabled_follows_config_not_the_credential() {
        let mut cfg = Config::default();
        cfg.zai.enabled = false;
        let set = |name: &str| name == "ZAI_API_KEY";
        let zai = row(
            &statuses_with(&cfg, &probes(&set, &|_| false, &|_| false)),
            "zai",
        );
        assert!(!zai.enabled, "switched off in config");
        assert!(zai.configured, "but its key is still there");
    }

    /// Command Code's search list includes pi's shared keystore, which exists
    /// whenever the user signed pi into any provider. Existence of that file is
    /// not a Command Code login — only a live credential inside it is, the same
    /// answer the fetch's own resolver gives.
    #[test]
    fn a_shared_harness_keystore_is_not_a_commandcode_login() {
        let mut cfg = Config::default();
        cfg.commandcode.auth_paths = Some(vec![PathBuf::from("/home/x/.pi/agent/auth.json")]);

        let keystore_only = statuses_with(&cfg, &probes(&|_| false, &|_| true, &|_| false));
        assert!(
            !row(&keystore_only, "commandcode").configured,
            "a pi login for another provider is not a Command Code login"
        );

        let signed_in = statuses_with(&cfg, &probes(&|_| false, &|_| true, &|_| true));
        assert!(row(&signed_in, "commandcode").configured);
    }

    /// A custom `auth_paths` install is configured by its own files, not by the
    /// platform defaults.
    #[test]
    fn commandcode_configuration_follows_the_configured_auth_paths() {
        let mut cfg = Config::default();
        let custom = PathBuf::from("/opt/commandcode/auth.json");
        cfg.commandcode.auth_paths = Some(vec![custom.clone()]);

        let rows = statuses_with(
            &cfg,
            &probes(&|_| false, &|_| false, &|paths| paths == [custom.clone()]),
        );
        assert!(row(&rows, "commandcode").configured);
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
        // The macOS menu bar decides a vendor's default state solely by
        // `enabled`, so the wire name is frontend contract: a serde rename
        // here would silently empty its selector, failing no Swift test.
        assert_eq!(vendors[0]["enabled"], true);
        assert_eq!(vendors[0]["configured"], false);
        assert_eq!(vendors[0]["short_name"], "cld");
        let agy = vendors
            .iter()
            .find(|v| v["id"] == "antigravity")
            .expect("antigravity is in the report");
        assert_eq!(agy["kind"], "local");
        assert_eq!(agy["needs_credential"], false);
    }
}
