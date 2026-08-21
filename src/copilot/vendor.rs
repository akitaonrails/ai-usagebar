//! GitHub Copilot renderer — primary bar text plus a three-pool tooltip.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::usage::{CopilotPool, CopilotSnapshot};
use crate::vendor::{RenderOpts, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{copilot_headline}";
const DEFAULT_ICON: &str = "";

pub fn build_placeholders(
    snap: &CopilotSnapshot,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let reset = countdown::format(snap.reset_at, now);
    let primary_pct = snap.primary_pct();
    let secondary_pct = snap.secondary_pct();
    placeholders(vec![
        ("icon", DEFAULT_ICON.to_string()),
        ("vendor_short", "ghc".to_string()),
        ("plan", format!("Copilot {}", snap.plan)),
        (
            "session_pct",
            primary_pct
                .map(|value| value.to_string())
                .unwrap_or_else(|| "0".into()),
        ),
        ("session_reset", reset.clone()),
        (
            "weekly_pct",
            secondary_pct
                .map(|value| value.to_string())
                .unwrap_or_else(|| "0".into()),
        ),
        ("weekly_reset", reset.clone()),
        ("copilot_login", snap.login.clone()),
        ("copilot_plan", snap.plan.clone()),
        ("copilot_reset", reset),
        ("copilot_headline", headline(snap)),
        (
            "copilot_primary_pct",
            primary_pct
                .map(|value| value.to_string())
                .unwrap_or_default(),
        ),
        ("copilot_chat_state", pool_state(&snap.chat).into()),
        ("copilot_chat_pct", pool_pct(&snap.chat)),
        ("copilot_chat_remaining", pool_remaining(&snap.chat)),
        ("copilot_chat_entitlement", pool_entitlement(&snap.chat)),
        (
            "copilot_completions_state",
            pool_state(&snap.completions).into(),
        ),
        ("copilot_completions_pct", pool_pct(&snap.completions)),
        (
            "copilot_completions_remaining",
            pool_remaining(&snap.completions),
        ),
        (
            "copilot_completions_entitlement",
            pool_entitlement(&snap.completions),
        ),
        (
            "copilot_premium_state",
            pool_state(&snap.premium_interactions).into(),
        ),
        ("copilot_premium_pct", pool_pct(&snap.premium_interactions)),
        (
            "copilot_premium_remaining",
            pool_remaining(&snap.premium_interactions),
        ),
        (
            "copilot_premium_entitlement",
            pool_entitlement(&snap.premium_interactions),
        ),
    ])
}

pub fn severity(snap: &CopilotSnapshot) -> PaceSeverity {
    match &snap.premium_interactions {
        CopilotPool::Metered { percent_used, .. } => severity_for(*percent_used),
        CopilotPool::Unlimited | CopilotPool::NotApplicable => {
            [snap.chat.percent_used(), snap.completions.percent_used()]
                .into_iter()
                .flatten()
                .max()
                .map(severity_for)
                .unwrap_or(PaceSeverity::Low)
        }
    }
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &CopilotSnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let class = Class::from(severity(snap));
    let format = opts
        .format
        .clone()
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string());
    let values = build_placeholders(snap, now);

    let mut text = substitute(&format, &values);
    if outcome.stale {
        text.push_str(" ⏸");
    }

    let wrapper_color = severity_color(severity(snap), theme).to_string();
    let icon_prefix = match opts.icon.as_deref() {
        Some(icon) if !icon.is_empty() => format!("{icon} "),
        _ => String::new(),
    };
    let bar_text = color_span(&wrapper_color, &format!("{icon_prefix}{text}"));
    let tooltip = if let Some(fmt) = opts.tooltip_format.as_deref() {
        substitute(fmt, &values)
    } else {
        render_tooltip(outcome, snap, theme, now)
    };

    WaybarOutput {
        text: bar_text,
        tooltip,
        class,
    }
}

fn headline(snap: &CopilotSnapshot) -> String {
    match &snap.premium_interactions {
        CopilotPool::Metered { percent_used, .. } => format!("{percent_used}%"),
        CopilotPool::Unlimited => "unlimited".into(),
        CopilotPool::NotApplicable => {
            let mut pools = Vec::new();
            for pool in [&snap.chat, &snap.completions] {
                match pool {
                    CopilotPool::Metered { percent_used, .. } => {
                        pools.push(percent_used.to_string())
                    }
                    CopilotPool::Unlimited => pools.push("∞".into()),
                    CopilotPool::NotApplicable => {}
                }
            }
            match pools.len() {
                0 => "n/a".into(),
                1 => format!("{}%", pools[0]),
                _ => format!("{}·{}%", pools[0], pools[1]),
            }
        }
    }
}

fn pool_state(pool: &CopilotPool) -> &'static str {
    match pool {
        CopilotPool::Metered { .. } => "metered",
        CopilotPool::Unlimited => "unlimited",
        CopilotPool::NotApplicable => "n/a",
    }
}

fn pool_pct(pool: &CopilotPool) -> String {
    match pool {
        CopilotPool::Metered { percent_used, .. } => percent_used.to_string(),
        CopilotPool::Unlimited | CopilotPool::NotApplicable => String::new(),
    }
}

fn pool_remaining(pool: &CopilotPool) -> String {
    match pool {
        CopilotPool::Metered { remaining, .. } => remaining.to_string(),
        CopilotPool::Unlimited | CopilotPool::NotApplicable => String::new(),
    }
}

fn pool_entitlement(pool: &CopilotPool) -> String {
    match pool {
        CopilotPool::Metered { entitlement, .. } => entitlement.to_string(),
        CopilotPool::Unlimited | CopilotPool::NotApplicable => String::new(),
    }
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snap: &CopilotSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let mut lines = vec![
        TooltipLine::Center(format!(
            "<span font_weight='bold' foreground='{}'>GitHub Copilot {}</span>",
            theme.blue,
            escape(&snap.plan)
        )),
        TooltipLine::Sep,
        TooltipLine::Body(String::new()),
        TooltipLine::Body(format!(
            " <span foreground='{}'>  󰊤  {}</span>",
            theme.dim,
            escape(&snap.login)
        )),
        TooltipLine::Body(String::new()),
    ];

    pool_lines(
        &mut lines,
        theme,
        "Premium requests",
        &snap.premium_interactions,
    );
    lines.push(TooltipLine::Body(String::new()));
    pool_lines(&mut lines, theme, "Chat", &snap.chat);
    lines.push(TooltipLine::Body(String::new()));
    pool_lines(&mut lines, theme, "Completions", &snap.completions);

    lines.push(TooltipLine::Body(String::new()));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{}'>  󰃰  Resets {}</span>",
        theme.dim,
        escape(&countdown::format(snap.reset_at, now))
    )));

    if let Some((code, msg)) = outcome.last_error.as_ref() {
        let (icon, color, header) = if *code == 0 {
            ("󰀪", theme.orange.as_str(), "Sync error".to_string())
        } else if *code >= 500 {
            ("󰅚", theme.red.as_str(), format!("HTTP {code}"))
        } else {
            ("󰀪", theme.orange.as_str(), format!("HTTP {code}"))
        };
        lines.push(TooltipLine::Body(String::new()));
        lines.push(TooltipLine::Sep);
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{color}'>  {icon}  {header}</span>"
        )));
        lines.push(TooltipLine::Body(format!(
            "     <span foreground='{}'>{}</span>",
            theme.dim,
            escape(msg)
        )));
        if matches!(*code, 401 | 403) {
            lines.push(TooltipLine::Body(format!(
                "     <span foreground='{}'>run `gh auth login` or set [copilot] token/api_key</span>",
                theme.dim
            )));
            lines.push(TooltipLine::Body(format!(
                "     <span foreground='{}'>the GitHub account also needs Copilot access</span>",
                theme.dim
            )));
        }
    }

    let updated = updated_at_hm(now, outcome.cache_age);
    lines.push(TooltipLine::Body(String::new()));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{}'>  󰅐  Updated {updated}</span>",
        theme.dim,
    )));
    render_bordered(&lines, theme)
}

fn pool_lines(lines: &mut Vec<TooltipLine>, theme: &Theme, label: &str, pool: &CopilotPool) {
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{}'>  󰢻  {label}</span>",
        theme.fg,
    )));
    match pool {
        CopilotPool::Metered {
            entitlement,
            remaining,
            percent_used,
        } => {
            let color = severity_color(severity_for(*percent_used), theme);
            lines.push(TooltipLine::Body(format!(
                "   <span font_weight='bold' foreground='{color}'>{percent_used}%</span> used · {remaining} / {entitlement} left"
            )));
        }
        CopilotPool::Unlimited => lines.push(TooltipLine::Body(format!(
            "   <span foreground='{}'>Unlimited</span>",
            theme.dim,
        ))),
        CopilotPool::NotApplicable => lines.push(TooltipLine::Body(format!(
            "   <span foreground='{}'>Not included on this plan</span>",
            theme.dim,
        ))),
    }
}

impl From<FetchOutcome> for VendorOutcome {
    fn from(outcome: FetchOutcome) -> Self {
        Self {
            snapshot: crate::usage::VendorSnapshot::Copilot(outcome.snapshot),
            stale: outcome.stale,
            last_error: outcome.last_error,
            cache_age: outcome.cache_age,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 18, 12, 0, 0).unwrap()
    }

    fn snap() -> CopilotSnapshot {
        CopilotSnapshot {
            login: "octocat".into(),
            plan: "Business".into(),
            premium_interactions: CopilotPool::Metered {
                entitlement: 200,
                remaining: 121,
                percent_used: 40,
            },
            chat: CopilotPool::Metered {
                entitlement: 200,
                remaining: 121,
                percent_used: 40,
            },
            completions: CopilotPool::Metered {
                entitlement: 2000,
                remaining: 2000,
                percent_used: 0,
            },
            reset_at: Some(now() + chrono::Duration::days(14)),
        }
    }

    fn outcome(snapshot: CopilotSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Copilot(snapshot),
            stale: false,
            last_error: None,
            cache_age: Some(std::time::Duration::from_secs(10)),
        }
    }

    fn opts() -> RenderOpts {
        RenderOpts {
            format: None,
            tooltip_format: None,
            icon: None,
            pace_tolerance: 5,
            format_pace_color: false,
            tooltip_pace_pts: false,
        }
    }

    #[test]
    fn headline_uses_premium_pool_when_present() {
        let snapshot = snap();
        let out = render(
            &outcome(snapshot.clone()),
            &snapshot,
            &Theme::default(),
            &opts(),
            now(),
        );
        assert!(out.text.contains("40%"));
    }

    #[test]
    fn tooltip_breaks_out_all_three_pools() {
        let snapshot = snap();
        let out = render(
            &outcome(snapshot.clone()),
            &snapshot,
            &Theme::default(),
            &opts(),
            now(),
        );
        assert!(out.tooltip.contains("GitHub Copilot Business"));
        assert!(out.tooltip.contains("Premium requests"));
        assert!(out.tooltip.contains("Chat"));
        assert!(out.tooltip.contains("Completions"));
        assert!(out.tooltip.contains("14d"));
    }

    #[test]
    fn not_applicable_premium_falls_back_to_chat_and_is_not_critical() {
        let mut snapshot = snap();
        snapshot.premium_interactions = CopilotPool::NotApplicable;
        snapshot.chat = CopilotPool::Metered {
            entitlement: 200,
            remaining: 5,
            percent_used: 98,
        };
        let out = render(
            &outcome(snapshot.clone()),
            &snapshot,
            &Theme::default(),
            &opts(),
            now(),
        );
        assert_eq!(severity(&snapshot), PaceSeverity::Critical);
        assert!(out.text.contains("98") || out.text.contains("98·0%"));
        assert!(out.tooltip.contains("Not included on this plan"));
    }

    #[test]
    fn unlimited_premium_is_distinctly_labeled() {
        let mut snapshot = snap();
        snapshot.premium_interactions = CopilotPool::Unlimited;
        let out = render(
            &outcome(snapshot.clone()),
            &snapshot,
            &Theme::default(),
            &opts(),
            now(),
        );
        assert_eq!(severity(&snapshot), PaceSeverity::Low);
        assert!(out.text.contains("unlimited"));
        assert!(out.tooltip.contains("Unlimited"));
    }

    #[test]
    fn auth_failures_add_gh_hint_lines() {
        let snapshot = snap();
        let mut outcome = outcome(snapshot.clone());
        outcome.last_error = Some((403, "GitHub Copilot authentication failed".into()));
        let out = render(&outcome, &snapshot, &Theme::default(), &opts(), now());
        assert!(out.tooltip.contains("gh auth login"));
        assert!(out.tooltip.contains("needs Copilot access"));
    }

    #[test]
    fn custom_tooltip_uses_placeholders() {
        let snapshot = snap();
        let mut render_opts = opts();
        render_opts.tooltip_format = Some(
            "{copilot_plan}:{copilot_premium_pct}:{copilot_chat_state}:{copilot_reset}".into(),
        );
        let out = render(
            &outcome(snapshot.clone()),
            &snapshot,
            &Theme::default(),
            &render_opts,
            now(),
        );
        assert_eq!(out.tooltip, "Business:40:metered:14d 0h");
    }
}
