//! Command-line interface — claudebar-compatible flags plus the new
//! local-testing additions (`--pretty`, `--watch`, `--json`).
//!
//! Mirrors claudebar:54-93. The defaults are identical so existing waybar
//! configs that invoke `claudebar ...` can be retargeted to
//! `ai-usagebar --vendor anthropic ...` without changing any flags.

use clap::{Parser, ValueEnum};

#[derive(Parser, Debug, Clone)]
#[command(
    name = "ai-usagebar",
    version,
    args_conflicts_with_subcommands = true,
    about = "Waybar widget and terminal dashboard for multi-provider AI plan usage",
    long_about = "\
Drop-in replacement for `claudebar` with multi-vendor support.

Output modes:
  - Default: Waybar JSON ({text, tooltip, class}). Used when stdout is piped.
  - --pretty: human-readable terminal output for local testing. Auto-enabled
    when stdout is a TTY, so just running `ai-usagebar --vendor anthropic`
    in a terminal Does The Right Thing.
  - --watch N: like --pretty but refreshes every N seconds, clearing the screen
    between ticks. Useful while iterating on `--format` or `--tooltip-format`.
  - --json: force JSON output even when stdout is a TTY (for scripting)."
)]
pub struct Cli {
    /// Which vendor to query. When omitted, reads `[ui] primary` from
    /// `~/.config/ai-usagebar/config.toml`; falls back to `anthropic` if
    /// neither is set.
    #[arg(long, value_enum)]
    pub vendor: Option<Vendor>,

    /// Optional icon prepended to the bar text (Nerd Font glyph / emoji /
    /// Pango span). claudebar `--icon`.
    #[arg(long)]
    pub icon: Option<String>,

    /// Bar-text format string with `{placeholder}` substitutions. Defaults to
    /// a vendor-specific format (e.g. `{session_pct}% · {session_reset}` for
    /// Anthropic, `{kimi_weekly_pct}%` for Kimi).
    #[arg(long)]
    pub format: Option<String>,

    /// Custom tooltip format. Overrides the default bordered tooltip when
    /// set; identical placeholder set as `--format`.
    #[arg(long)]
    pub tooltip_format: Option<String>,

    /// Tolerance band (in percentage points) for ratio-based pacing icons.
    #[arg(long, default_value_t = 5)]
    pub pace_tolerance: u32,

    /// Color pace placeholders individually per window (instead of the
    /// global usage-based color). Claudebar `--format-pace-color`.
    #[arg(long)]
    pub format_pace_color: bool,

    /// Use point-based pacing in the tooltip's pace column (vs ratio-based).
    /// Also enables an elapsed-position marker on the tooltip progress bars.
    /// Claudebar `--tooltip-pace-pts`.
    #[arg(long)]
    pub tooltip_pace_pts: bool,

    /// Override the low-usage color (#RRGGBB).
    #[arg(long)]
    pub color_low: Option<String>,
    /// Override the mid-usage color (#RRGGBB).
    #[arg(long)]
    pub color_mid: Option<String>,
    /// Override the high-usage color (#RRGGBB).
    #[arg(long)]
    pub color_high: Option<String>,
    /// Override the critical-usage color (#RRGGBB).
    #[arg(long)]
    pub color_critical: Option<String>,

    /// Render human-readable terminal output (ANSI colors + box drawing)
    /// instead of Waybar JSON. Auto-on when stdout is a TTY.
    #[arg(long)]
    pub pretty: bool,

    /// Force JSON output even on a TTY (useful when piping into `jq` from
    /// an interactive shell).
    #[arg(long, conflicts_with = "pretty")]
    pub json: bool,

    /// Re-render every N seconds, clearing the screen between ticks. Implies
    /// `--pretty`. Press Ctrl-C to exit.
    #[arg(long, value_name = "SECS")]
    pub watch: Option<u64>,

    /// Cycle the persisted "active vendor" forward and exit. Wire to
    /// Waybar's `on-scroll-up` to scroll-cycle through enabled vendors.
    /// Sends SIGRTMIN+13 to waybar afterwards so the bar refreshes
    /// immediately rather than waiting for the next interval tick.
    #[arg(long, conflicts_with_all = ["cycle_prev", "watch", "pretty", "json"])]
    pub cycle_next: bool,

    /// Cycle backwards. Wire to `on-scroll-down`.
    #[arg(long, conflicts_with_all = ["cycle_next", "watch", "pretty", "json"])]
    pub cycle_prev: bool,

    /// Override the cache directory (default: ~/.cache/ai-usagebar/<vendor>).
    /// Give each instance its own directory to track multiple accounts of
    /// the same vendor side by side — see "Multiple accounts" in the README.
    #[arg(long, value_name = "DIR")]
    pub cache_dir: Option<std::path::PathBuf>,

    /// Override the Anthropic credentials file (default:
    /// ~/.claude/.credentials.json, or `[anthropic] credentials_path` from
    /// config). Only the Anthropic vendor reads this flag. Combine with
    /// --cache-dir to track multiple Claude accounts — see "Multiple
    /// accounts" in the README.
    #[arg(long, value_name = "FILE")]
    pub creds_path: Option<std::path::PathBuf>,

    /// Select a named Anthropic account from `[[anthropic.accounts]]` in
    /// config (issue #14). Without it, `--vendor anthropic` uses the default
    /// account — the singular `[anthropic] credentials_path` — with unchanged
    /// output and cache path. Anthropic only; conflicts with the lower-level
    /// `--creds-path` (they both name a credentials file).
    #[arg(long, value_name = "LABEL", conflicts_with = "creds_path")]
    pub account: Option<String>,

    /// Read `--account <LABEL>`'s usage from the Claude **Desktop app's** own
    /// token instead of a `claude` CLI credential — a saved
    /// `~/.claude-acc/profiles/<LABEL>` account, no CLI login required (macOS).
    /// This is how the menu bar shows Desktop accounts in its overview.
    #[arg(long, requires = "account")]
    pub desktop: bool,

    /// Administrative command. Omit it to run the normal usage widget.
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum Command {
    /// Manage named Claude (Anthropic) accounts.
    Account {
        #[command(subcommand)]
        action: AccountAction,
    },

    /// Quota and time-to-reset for every configured vendor and account.
    Usage {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },

    /// Read or update settings for native desktop frontends.
    Settings {
        #[command(subcommand)]
        action: SettingsAction,
    },

    /// Authenticate a provider without starting the widget.
    Auth {
        #[command(subcommand)]
        provider: AuthProvider,
    },
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum AuthProvider {
    Nous {
        #[command(subcommand)]
        action: NousAuthAction,
    },
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum NousAuthAction {
    /// Start the Nous Research OAuth device flow.
    Login,
    /// Remove only the Nous Research credential.
    Logout,
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum SettingsAction {
    /// Print a non-secret JSON settings description.
    Show,

    /// Apply one JSON settings patch read from standard input.
    Apply,
}

#[derive(clap::Subcommand, Debug, Clone)]
pub enum AccountAction {
    /// Register an isolated account and open Claude Code to sign it in.
    Add {
        /// Stable name used by `--account`, the TUI, and desktop apps.
        label: String,

        /// Only register the account; do not launch interactive login.
        #[arg(long, conflicts_with = "desktop")]
        no_login: bool,

        /// Capture a Claude **Desktop app** account under this label instead
        /// of a `claude` CLI one (macOS). The app has a single login slot, so
        /// this signs it out, waits for you to sign in as the new account, and
        /// saves what it writes. Your current login is restored if you cancel.
        #[arg(long)]
        desktop: bool,

        /// E-mail to label a `--desktop` account with. Asked for at the prompt
        /// if omitted; purely cosmetic, and skipped when not interactive.
        #[arg(long, requires = "desktop")]
        email: Option<String>,

        /// Skip the confirmation before signing the Desktop app out.
        #[arg(short = 'y', long, requires = "desktop")]
        yes: bool,
    },

    /// Show which Claude account the Desktop app and the `claude` CLI use.
    Status {
        /// Machine-readable output, consumed by the macOS menu bar.
        #[arg(long)]
        json: bool,
    },

    /// Make <LABEL> the active Claude account (macOS).
    Switch {
        /// Account to switch to. Desktop profiles come from claude-acc's store;
        /// CLI accounts from `[[anthropic.accounts]]` / `accounts_dir`.
        label: String,

        /// Only switch the Claude Desktop app. Neither flag switches both.
        #[arg(long)]
        desktop: bool,

        /// Only switch the `claude` CLI's default login.
        #[arg(long)]
        cli: bool,

        /// Report what would change and exit without touching anything.
        #[arg(long)]
        dry_run: bool,

        /// Skip the confirmation before quitting the Claude Desktop app.
        #[arg(short = 'y', long)]
        yes: bool,

        /// Overwrite a `claude` CLI login that belongs to no managed account.
        /// That login cannot be saved first, so this discards it.
        #[arg(long)]
        force: bool,

        /// Keep `bridge-state.json` rather than clearing it. Diagnostic only:
        /// a stale remote-control session id breaks `/remote-control`.
        #[arg(long)]
        keep_bridge: bool,

        /// Also archive the whole session tree, as claude-acc does. Off by
        /// default because the history merge is additive.
        #[arg(long)]
        backup_sessions: bool,

        /// Rollback archives to retain.
        #[arg(long, default_value_t = 10)]
        keep_backups: usize,

        /// Confirm that this type-scoped conflict key, deleted in one account
        /// but still held by another, should be removed everywhere. Repeatable.
        /// Supplying any suppresses the interactive prompt — keys not listed
        /// are kept — which is how the macOS menu bar passes an answered dialog
        /// through.
        /// `account status --json` lists the candidates as `deletion_conflicts`.
        #[arg(long, value_name = "KEY")]
        delete_conflict: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum Vendor {
    Anthropic,
    #[value(name = "anthropic_api")]
    AnthropicApi,
    Openai,
    Zai,
    Openrouter,
    Deepseek,
    Kimi,
    Kilo,
    Novita,
    Moonshot,
    Grok,
    Supergrok,
    Antigravity,
    Cursor,
    Copilot,
    Minimax,
    Kiro,
    #[value(name = "nous")]
    NousResearch,
    #[value(name = "opencode-go")]
    OpenCodeGo,
}

impl Vendor {
    pub fn to_id(self) -> crate::vendor::VendorId {
        match self {
            Vendor::Anthropic => crate::vendor::VendorId::Anthropic,
            Vendor::AnthropicApi => crate::vendor::VendorId::AnthropicApi,
            Vendor::Openai => crate::vendor::VendorId::Openai,
            Vendor::Zai => crate::vendor::VendorId::Zai,
            Vendor::Openrouter => crate::vendor::VendorId::Openrouter,
            Vendor::Deepseek => crate::vendor::VendorId::Deepseek,
            Vendor::Kimi => crate::vendor::VendorId::Kimi,
            Vendor::Kilo => crate::vendor::VendorId::Kilo,
            Vendor::Novita => crate::vendor::VendorId::Novita,
            Vendor::Moonshot => crate::vendor::VendorId::Moonshot,
            Vendor::Grok => crate::vendor::VendorId::Grok,
            Vendor::Supergrok => crate::vendor::VendorId::Supergrok,
            Vendor::Antigravity => crate::vendor::VendorId::Antigravity,
            Vendor::Cursor => crate::vendor::VendorId::Cursor,
            Vendor::Copilot => crate::vendor::VendorId::Copilot,
            Vendor::Minimax => crate::vendor::VendorId::Minimax,
            Vendor::Kiro => crate::vendor::VendorId::Kiro,
            Vendor::NousResearch => crate::vendor::VendorId::NousResearch,
            Vendor::OpenCodeGo => crate::vendor::VendorId::OpenCodeGo,
        }
    }
}

impl Cli {
    /// Whether the selected vendor came from an explicit `--vendor` opt-in.
    pub fn has_explicit_vendor(&self) -> bool {
        self.vendor.is_some()
    }

    /// Resolve the vendor with full precedence:
    ///   1. explicit `--vendor` (highest)
    ///   2. persisted scroll-cycle state (`~/.cache/ai-usagebar/active_vendor`)
    ///   3. `[ui] primary` from config
    ///   4. anthropic (lowest)
    ///
    /// This reads the persisted scroll-cycle state from disk via
    /// [`crate::active::read`]. The pure precedence logic lives in
    /// [`Cli::resolve_vendor_with`] so it can be unit-tested without touching
    /// `~/.cache/ai-usagebar/active_vendor`.
    pub fn resolved_vendor(&self, config: &crate::config::Config) -> Vendor {
        // Only consult the scroll-cycle state file when it could actually
        // matter. An explicit `--vendor` wins outright (precedence #1), so we
        // skip the disk read entirely in that case — preserving the original
        // short-circuit and keeping the documented `--vendor` widget config off
        // the `active_vendor` read path.
        let active = if self.has_explicit_vendor() {
            None
        } else {
            crate::active::read()
        };
        self.resolve_vendor_with(config, active)
    }

    /// Pure precedence resolution given an explicit scroll-cycle `active`
    /// override (i.e. whatever [`crate::active::read`] returned). Split out
    /// from the disk read so tests exercise the precedence rules hermetically
    /// instead of depending on the developer's real `active_vendor` file.
    pub fn resolve_vendor_with(
        &self,
        config: &crate::config::Config,
        active: Option<crate::vendor::VendorId>,
    ) -> Vendor {
        if let Some(v) = self.vendor {
            return v;
        }
        if let Some(id) = active
            && config.is_enabled(id)
        {
            return id_to_vendor(id);
        }
        if let Some(id) = config.ui.primary
            && config.is_enabled(id)
        {
            return id_to_vendor(id);
        }
        if config.is_enabled(crate::vendor::VendorId::Anthropic) {
            return Vendor::Anthropic;
        }
        config
            .enabled_vendors()
            .into_iter()
            .next()
            .map(id_to_vendor)
            // A completely disabled configuration has no enabled choice; keep
            // the historic final fallback rather than rejecting widget startup.
            .unwrap_or(Vendor::Anthropic)
    }
}

fn id_to_vendor(id: crate::vendor::VendorId) -> Vendor {
    match id {
        crate::vendor::VendorId::Anthropic => Vendor::Anthropic,
        crate::vendor::VendorId::AnthropicApi => Vendor::AnthropicApi,
        crate::vendor::VendorId::Openai => Vendor::Openai,
        crate::vendor::VendorId::Zai => Vendor::Zai,
        crate::vendor::VendorId::Openrouter => Vendor::Openrouter,
        crate::vendor::VendorId::Deepseek => Vendor::Deepseek,
        crate::vendor::VendorId::Kimi => Vendor::Kimi,
        crate::vendor::VendorId::Kilo => Vendor::Kilo,
        crate::vendor::VendorId::Novita => Vendor::Novita,
        crate::vendor::VendorId::Moonshot => Vendor::Moonshot,
        crate::vendor::VendorId::Grok => Vendor::Grok,
        crate::vendor::VendorId::Supergrok => Vendor::Supergrok,
        crate::vendor::VendorId::Antigravity => Vendor::Antigravity,
        crate::vendor::VendorId::Cursor => Vendor::Cursor,
        crate::vendor::VendorId::Copilot => Vendor::Copilot,
        crate::vendor::VendorId::Minimax => Vendor::Minimax,
        crate::vendor::VendorId::Kiro => Vendor::Kiro,
        crate::vendor::VendorId::NousResearch => Vendor::NousResearch,
        crate::vendor::VendorId::OpenCodeGo => Vendor::OpenCodeGo,
    }
}

impl Cli {
    /// True when we should emit Waybar JSON. Default behavior: JSON when
    /// stdout is piped, pretty when on a TTY (unless `--json` is set).
    pub fn output_json(&self) -> bool {
        if self.json {
            return true;
        }
        if self.pretty || self.watch.is_some() {
            return false;
        }
        // Auto-detect: emit pretty when stdout is a TTY.
        !is_stdout_tty()
    }
}

fn is_stdout_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Parser, error::ErrorKind};

    #[test]
    fn version_flags_report_the_crate_version() {
        let expected = format!("ai-usagebar {}\n", env!("CARGO_PKG_VERSION"));

        for flag in ["--version", "-V"] {
            let err = Cli::try_parse_from(["ai-usagebar", flag])
                .expect_err("a version flag exits through clap's display path");
            assert_eq!(err.kind(), ErrorKind::DisplayVersion, "flag: {flag}");
            assert_eq!(err.to_string(), expected, "flag: {flag}");
        }
    }

    #[test]
    fn usage_subcommand_parses_machine_readable_mode() {
        let cli = Cli::parse_from(["ai-usagebar", "usage", "--json"]);
        assert!(matches!(cli.command, Some(Command::Usage { json: true })));
    }

    #[test]
    fn new_vendor_values_and_auth_commands_parse_exactly() {
        let nous = Cli::parse_from(["ai-usagebar", "--vendor", "nous"]);
        assert_eq!(nous.vendor, Some(Vendor::NousResearch));
        let opencode = Cli::parse_from(["ai-usagebar", "--vendor", "opencode-go"]);
        assert_eq!(opencode.vendor, Some(Vendor::OpenCodeGo));
        let login = Cli::parse_from(["ai-usagebar", "auth", "nous", "login"]);
        assert!(matches!(login.command, Some(Command::Auth { .. })));
    }

    #[test]
    fn settings_subcommands_are_additive_and_take_no_widget_flags() {
        let show = Cli::parse_from(["ai-usagebar", "settings", "show"]);
        assert!(matches!(
            show.command,
            Some(Command::Settings {
                action: SettingsAction::Show
            })
        ));

        let apply = Cli::parse_from(["ai-usagebar", "settings", "apply"]);
        assert!(matches!(
            apply.command,
            Some(Command::Settings {
                action: SettingsAction::Apply
            })
        ));

        assert!(
            Cli::try_parse_from(["ai-usagebar", "--vendor", "kimi", "settings", "show",]).is_err()
        );
    }

    #[test]
    fn defaults_match_claudebar() {
        let cli = Cli::parse_from(["ai-usagebar"]);
        assert_eq!(cli.vendor, None);
        // Without explicit --vendor, no scroll-cycle override, and default
        // config, resolve to anthropic. Use `resolve_vendor_with(.., None)`
        // rather than `resolved_vendor` so the test never reads the real
        // ~/.cache/ai-usagebar/active_vendor file.
        let cfg = crate::config::Config::default();
        assert_eq!(cli.resolve_vendor_with(&cfg, None), Vendor::Anthropic);
        assert_eq!(cli.pace_tolerance, 5);
        assert!(cli.format.is_none());
        assert!(cli.tooltip_format.is_none());
        assert!(cli.icon.is_none());
        assert!(!cli.format_pace_color);
        assert!(!cli.tooltip_pace_pts);
        assert!(!cli.pretty);
        assert!(!cli.json);
        assert!(cli.watch.is_none());
        assert!(cli.command.is_none());
    }

    #[test]
    fn account_add_subcommand_parses_without_widget_flags() {
        let cli = Cli::parse_from(["ai-usagebar", "account", "add", "work", "--no-login"]);
        assert!(matches!(
            cli.command,
            Some(Command::Account {
                action: AccountAction::Add {
                    ref label,
                    no_login: true,
                    desktop: false,
                    ..
                }
            }) if label == "work"
        ));
    }

    /// The two halves of `add` capture different things and cannot be combined:
    /// `--no-login` skips a `claude` login the Desktop capture never runs.
    #[test]
    fn account_add_desktop_takes_an_email_and_rejects_no_login() {
        let cli = Cli::parse_from([
            "ai-usagebar",
            "account",
            "add",
            "work",
            "--desktop",
            "--email",
            "a@b.test",
            "-y",
        ]);
        assert!(matches!(
            cli.command,
            Some(Command::Account {
                action: AccountAction::Add {
                    desktop: true,
                    yes: true,
                    email: Some(ref email),
                    ..
                }
            }) if email == "a@b.test"
        ));
        assert!(
            Cli::try_parse_from([
                "ai-usagebar",
                "account",
                "add",
                "w",
                "--desktop",
                "--no-login"
            ])
            .is_err()
        );
        // --email / -y only mean something for the Desktop capture.
        assert!(
            Cli::try_parse_from(["ai-usagebar", "account", "add", "w", "--email", "a@b.test"])
                .is_err()
        );
    }

    #[test]
    fn account_switch_defaults_to_both_surfaces() {
        let cli = Cli::parse_from(["ai-usagebar", "account", "switch", "work", "--dry-run"]);
        assert!(matches!(
            cli.command,
            Some(Command::Account {
                action: AccountAction::Switch {
                    ref label,
                    desktop: false,
                    cli: false,
                    dry_run: true,
                    keep_backups: 10,
                    ..
                }
            }) if label == "work"
        ));
    }

    #[test]
    fn account_subcommand_rejects_ignored_widget_flags() {
        assert!(
            Cli::try_parse_from([
                "ai-usagebar",
                "--vendor",
                "anthropic",
                "account",
                "add",
                "work",
            ])
            .is_err()
        );
    }

    #[test]
    fn multi_account_flags_are_stable_api() {
        // --cache-dir and --creds-path are the documented multi-account
        // mechanism (README "Multiple accounts") since they were promoted
        // from hidden debug flags. Renaming either is a breaking change.
        let cli = Cli::parse_from([
            "ai-usagebar",
            "--vendor",
            "anthropic",
            "--cache-dir",
            "/tmp/acct-a",
            "--creds-path",
            "/tmp/acct-a/credentials.json",
        ]);
        assert_eq!(
            cli.cache_dir.as_deref(),
            Some(std::path::Path::new("/tmp/acct-a"))
        );
        assert_eq!(
            cli.creds_path.as_deref(),
            Some(std::path::Path::new("/tmp/acct-a/credentials.json"))
        );
    }

    #[test]
    fn primary_from_config_wins_when_vendor_unset() {
        // No --vendor and no scroll-cycle override → [ui] primary wins.
        let cli = Cli::parse_from(["ai-usagebar"]);
        let mut cfg = crate::config::Config::default();
        cfg.ui.primary = Some(crate::vendor::VendorId::Openrouter);
        assert_eq!(cli.resolve_vendor_with(&cfg, None), Vendor::Openrouter);
    }

    #[test]
    fn explicit_vendor_overrides_everything() {
        // Explicit --vendor beats BOTH a persisted scroll-cycle override and
        // [ui] primary.
        let cli = Cli::parse_from(["ai-usagebar", "--vendor", "zai"]);
        let mut cfg = crate::config::Config::default();
        cfg.ui.primary = Some(crate::vendor::VendorId::Openrouter);
        let active = Some(crate::vendor::VendorId::Openai);
        assert_eq!(cli.resolve_vendor_with(&cfg, active), Vendor::Zai);
    }

    #[test]
    fn vendor_kimi_parses_to_kimi_variant() {
        let cli = Cli::parse_from(["ai-usagebar", "--vendor", "kimi"]);
        assert_eq!(cli.vendor, Some(Vendor::Kimi));
        assert_eq!(cli.vendor.unwrap().to_id(), crate::vendor::VendorId::Kimi);
    }

    #[test]
    fn vendor_anthropic_api_uses_the_documented_slug() {
        let cli = Cli::parse_from(["ai-usagebar", "--vendor", "anthropic_api"]);
        assert_eq!(cli.vendor, Some(Vendor::AnthropicApi));
        assert_eq!(
            cli.vendor.unwrap().to_id(),
            crate::vendor::VendorId::AnthropicApi
        );
    }

    #[test]
    fn disabled_kimi_primary_falls_back_to_an_enabled_vendor() {
        let cli = Cli::parse_from(["ai-usagebar"]);
        let mut cfg = crate::config::Config::default();
        cfg.ui.primary = Some(crate::vendor::VendorId::Kimi);
        assert_eq!(cli.resolve_vendor_with(&cfg, None), Vendor::Anthropic);
    }

    #[test]
    fn explicit_kimi_remains_an_opt_in_override_when_disabled() {
        let cli = Cli::parse_from(["ai-usagebar", "--vendor", "kimi"]);
        assert_eq!(
            cli.resolve_vendor_with(&crate::config::Config::default(), None),
            Vendor::Kimi
        );
    }

    #[test]
    fn active_override_wins_over_config_primary_when_enabled() {
        // Precedence rule #2: a persisted scroll-cycle vendor beats [ui]
        // primary, as long as it is still enabled.
        let cli = Cli::parse_from(["ai-usagebar"]);
        let mut cfg = crate::config::Config::default();
        cfg.ui.primary = Some(crate::vendor::VendorId::Openrouter);
        let active = Some(crate::vendor::VendorId::Zai);
        assert_eq!(cli.resolve_vendor_with(&cfg, active), Vendor::Zai);
    }

    #[test]
    fn disabled_active_override_falls_back_to_config_primary() {
        // A persisted active vendor the user has since disabled is skipped;
        // resolution falls through to [ui] primary.
        let cli = Cli::parse_from(["ai-usagebar"]);
        let mut cfg = crate::config::Config::default();
        cfg.zai.enabled = false;
        cfg.ui.primary = Some(crate::vendor::VendorId::Openrouter);
        let active = Some(crate::vendor::VendorId::Zai);
        assert_eq!(cli.resolve_vendor_with(&cfg, active), Vendor::Openrouter);
    }

    #[test]
    fn claudebar_compatible_flag_surface() {
        let cli = Cli::parse_from([
            "ai-usagebar",
            "--icon",
            "󰚩",
            "--format",
            "{session_pct}% · {session_reset}",
            "--tooltip-format",
            "S:{session_pct}",
            "--pace-tolerance",
            "10",
            "--format-pace-color",
            "--tooltip-pace-pts",
            "--color-low",
            "#50fa7b",
            "--color-mid",
            "#f1fa8c",
            "--color-high",
            "#ffb86c",
            "--color-critical",
            "#ff5555",
        ]);
        assert_eq!(cli.icon.as_deref(), Some("󰚩"));
        assert_eq!(
            cli.format.as_deref(),
            Some("{session_pct}% · {session_reset}")
        );
        assert_eq!(cli.tooltip_format.as_deref(), Some("S:{session_pct}"));
        assert_eq!(cli.pace_tolerance, 10);
        assert!(cli.format_pace_color);
        assert!(cli.tooltip_pace_pts);
        assert_eq!(cli.color_low.as_deref(), Some("#50fa7b"));
        assert_eq!(cli.color_critical.as_deref(), Some("#ff5555"));
    }

    #[test]
    fn pretty_and_json_conflict() {
        let res = Cli::try_parse_from(["ai-usagebar", "--pretty", "--json"]);
        assert!(res.is_err());
    }

    #[test]
    fn watch_disables_json_output() {
        let cli = Cli::parse_from(["ai-usagebar", "--watch", "5"]);
        assert_eq!(cli.watch, Some(5));
        assert!(!cli.output_json());
    }
}
