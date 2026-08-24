//! Tavily renderer — bar text + bordered Pango tooltip.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::usage::TavilySnapshot;
use crate::vendor::{RenderOpts, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::{FetchOutcome, SCHEMA_DRIFT_MESSAGE};

/// Presentation classification for Tavily's cached `(u16, String)` diagnostic.
/// Code zero has never meant HTTP; the stable schema marker lets renderers
/// distinguish an upstream response-shape change from other local failures
/// without changing the on-disk cache format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningKind {
    Http(u16),
    SchemaDrift,
    Other,
}

pub fn warning_kind(code: u16, message: &str) -> WarningKind {
    if code != 0 {
        WarningKind::Http(code)
    } else if message == SCHEMA_DRIFT_MESSAGE {
        WarningKind::SchemaDrift
    } else {
        WarningKind::Other
    }
}

/// Truthful bar headline: the plan percentage when the plan has a positive
/// limit, otherwise the raw credit count — never a fabricated percent.
pub fn headline(snap: &TavilySnapshot) -> String {
    match snap.plan_pct() {
        Some(pct) => format!("{pct}%"),
        None => format!("{} used", snap.plan_used),
    }
}

pub const DEFAULT_FORMAT: &str = "{tav_headline}";

pub fn build_placeholders(snap: &TavilySnapshot) -> HashMap<&'static str, String> {
    let plan = &snap.plan;
    let plan_pct = snap.plan_pct().map(|p| p.to_string());
    let unlimited = "unlimited".to_string();
    placeholders(vec![
        ("icon", "󰚩".to_string()),
        ("vendor_short", "tav".to_string()),
        // Cross-vendor aliases. Tavily has no rolling windows and no reset
        // timestamp, so the generic slots map to the billing-cycle plan
        // percentage when one exists and neutral values otherwise (the native
        // surfaces key off `vendor_short`, never turning these into fake bars).
        ("plan", plan.clone()),
        (
            "session_pct",
            plan_pct.clone().unwrap_or_else(|| "0".into()),
        ),
        ("session_reset", "—".to_string()),
        ("weekly_pct", plan_pct.clone().unwrap_or_else(|| "0".into())),
        ("weekly_reset", "—".to_string()),
        // Tavily-specific placeholders.
        ("tav_plan", plan.clone()),
        ("tav_headline", headline(snap)),
        ("tav_plan_pct", plan_pct.unwrap_or_else(|| "—".into())),
        ("tav_plan_used", snap.plan_used.to_string()),
        (
            "tav_plan_limit",
            snap.plan_limit
                .map(|limit| limit.to_string())
                .unwrap_or_else(|| unlimited.clone()),
        ),
        ("tav_payg_used", snap.payg_used.to_string()),
        (
            "tav_payg_limit",
            snap.payg_limit
                .map(|limit| limit.to_string())
                .unwrap_or_else(|| "—".into()),
        ),
        ("tav_key_used", snap.key_used.to_string()),
        (
            "tav_key_limit",
            snap.key_limit
                .map(|limit| limit.to_string())
                .unwrap_or_else(|| unlimited.clone()),
        ),
        ("tav_search", snap.search.to_string()),
        ("tav_extract", snap.extract.to_string()),
        ("tav_crawl", snap.crawl.to_string()),
        ("tav_map", snap.map.to_string()),
        ("tav_research", snap.research.to_string()),
    ])
}

pub fn severity(snap: &TavilySnapshot) -> PaceSeverity {
    severity_for(snap.plan_pct().unwrap_or(0))
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &TavilySnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let class = Class::from(severity(snap));
    let format = opts
        .format
        .clone()
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string());
    let values = build_placeholders(snap);
    // User formats are Pango markup after Waybar renders them. Escape API
    // strings there, while retaining raw values for the default tooltip (which
    // escapes exactly once at its markup insertion point).
    let mut pango_values = values.clone();
    for key in ["plan", "tav_plan"] {
        if let Some(value) = pango_values.get_mut(key) {
            *value = escape(value);
        }
    }

    let mut text = substitute(&format, &pango_values);
    if outcome.stale {
        text.push_str(" ⏸");
    }

    let wrapper_color = severity_color(severity(snap), theme).to_string();
    let icon_prefix = match opts.icon.as_deref() {
        Some(ic) if !ic.is_empty() => format!("{ic} "),
        _ => String::new(),
    };
    let bar_text = color_span(&wrapper_color, &format!("{icon_prefix}{text}"));

    let tooltip = if let Some(fmt) = opts.tooltip_format.as_deref() {
        substitute(fmt, &pango_values)
    } else {
        render_tooltip(outcome, snap, theme, now)
    };

    WaybarOutput {
        text: bar_text,
        tooltip,
        class,
    }
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snap: &TavilySnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let fg = &theme.fg;
    let color = severity_color(severity(snap), theme);

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>Tavily</span>"
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰣖  Plan</span>"
    )));
    match snap.plan_pct() {
        Some(pct) => lines.push(TooltipLine::Body(format!(
            "   <span font_weight='bold' foreground='{color}'>{plan}</span>  ({pct}%)",
            plan = escape(&snap.plan)
        ))),
        None => lines.push(TooltipLine::Body(format!(
            "   <span font_weight='bold' foreground='{color}'>{plan}</span>  (unlimited)",
            plan = escape(&snap.plan)
        ))),
    }
    // A zero or absent limit is "no cap" — never show "/ 0".
    let plan_limit = snap.plan_limit.filter(|limit| *limit > 0);
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>     {used} / {limit} used this cycle</span>",
        used = snap.plan_used,
        limit = plan_limit
            .map(|limit| limit.to_string())
            .unwrap_or_else(|| "∞".into())
    )));

    lines.push(TooltipLine::Body("".into()));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰦖  Pay-as-you-go</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        "   <span font_weight='bold' foreground='{color}'>{used}</span>  {limit}",
        used = snap.payg_used,
        limit = snap
            .payg_limit
            .map(|limit| format!("of {limit}"))
            .unwrap_or_else(|| "credits".into())
    )));

    lines.push(TooltipLine::Body("".into()));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰆓  This key</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        "   <span font_weight='bold' foreground='{color}'>{used}</span>  {limit}",
        used = snap.key_used,
        limit = snap
            .key_limit
            .map(|limit| format!("of {limit}"))
            .unwrap_or_else(|| "unlimited".into())
    )));

    lines.push(TooltipLine::Body("".into()));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰇀  Usage by endpoint</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>     search {search} · extract {extract}</span>",
        search = snap.search,
        extract = snap.extract
    )));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>     crawl {crawl} · map {map} · research {research}</span>",
        crawl = snap.crawl,
        map = snap.map,
        research = snap.research
    )));

    if let Some((code, msg)) = outcome.last_error.as_ref() {
        let (label, icon, ecolor) = match warning_kind(*code, msg) {
            WarningKind::SchemaDrift => (
                "Tavily API schema drift".to_string(),
                "󰅚",
                theme.red.as_str(),
            ),
            WarningKind::Other => ("Tavily error".to_string(), "󰅚", theme.red.as_str()),
            WarningKind::Http(code) if code >= 500 => {
                (format!("HTTP {code}"), "󰅚", theme.red.as_str())
            }
            WarningKind::Http(code) => (format!("HTTP {code}"), "󰀪", theme.orange.as_str()),
        };
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Sep);
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{ecolor}'>  {icon}  {label}</span>"
        )));
        if msg != &label {
            lines.push(TooltipLine::Body(format!(
                "     <span foreground='{dim}'>{}</span>",
                escape(msg)
            )));
        }
    }

    let updated = updated_at_hm(now, outcome.cache_age);
    lines.push(TooltipLine::Body("".into()));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  󰅐  Updated {updated}</span>"
    )));

    render_bordered(&lines, theme)
}

impl From<FetchOutcome> for VendorOutcome {
    fn from(o: FetchOutcome) -> Self {
        Self {
            snapshot: crate::usage::VendorSnapshot::Tavily(o.snapshot),
            stale: o.stale,
            last_error: o.last_error,
            cache_age: o.cache_age,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    fn sample_snap() -> TavilySnapshot {
        TavilySnapshot {
            plan: "Pro".into(),
            plan_used: 620,
            plan_limit: Some(1000),
            payg_used: 25,
            payg_limit: Some(100),
            key_used: 150,
            key_limit: Some(1000),
            search: 350,
            extract: 75,
            crawl: 50,
            map: 15,
            research: 10,
            scope_fingerprint: String::new(),
        }
    }

    fn sample_outcome(snap: TavilySnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Tavily(snap),
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
    fn default_render_shows_plan_percent_once() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.text.contains("62%"), "text: {}", out.text);
        assert_eq!(out.text.matches('%').count(), 1, "text: {}", out.text);
    }

    #[test]
    fn headline_is_truthful_without_a_plan_limit() {
        let mut snap = sample_snap();
        snap.plan_limit = None;
        assert_eq!(headline(&snap), "620 used");
        assert_eq!(build_placeholders(&snap)["session_pct"], "0");
        assert_eq!(build_placeholders(&snap)["tav_plan_pct"], "—");
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.text.contains("620 used"), "text: {}", out.text);
        assert!(!out.text.contains("%"), "text: {}", out.text);
    }

    #[test]
    fn severity_follows_plan_percent() {
        let mut snap = sample_snap();
        snap.plan_used = 950;
        assert_eq!(severity(&snap), PaceSeverity::Critical);
        snap.plan_used = 620;
        assert_eq!(severity(&snap), PaceSeverity::Mid);
        snap.plan_limit = None;
        assert_eq!(severity(&snap), PaceSeverity::Low);
    }

    #[test]
    fn placeholder_set_contains_all_keys() {
        let values = build_placeholders(&sample_snap());
        for key in [
            "vendor_short",
            "plan",
            "session_pct",
            "weekly_pct",
            "tav_plan",
            "tav_headline",
            "tav_plan_pct",
            "tav_plan_used",
            "tav_plan_limit",
            "tav_payg_used",
            "tav_payg_limit",
            "tav_key_used",
            "tav_key_limit",
            "tav_search",
            "tav_extract",
            "tav_crawl",
            "tav_map",
            "tav_research",
        ] {
            assert!(values.contains_key(key), "missing placeholder {key}");
        }
        assert_eq!(values["vendor_short"], "tav");
        assert_eq!(values["tav_plan_pct"], "62");
    }

    #[test]
    fn plan_is_pango_escaped() {
        let mut snap = sample_snap();
        snap.plan = "A&B <beta>".into();
        let outcome = sample_outcome(snap.clone());
        let mut o = opts();
        o.tooltip_format = Some("{tav_plan}".into());
        let out = render(&outcome, &snap, &Theme::default(), &o, now());
        assert_eq!(out.tooltip, "A&amp;B &lt;beta&gt;");
    }

    #[test]
    fn tooltip_includes_plan_payg_key_and_endpoint_breakdown() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.tooltip.contains("Tavily"));
        assert!(out.tooltip.contains("62%"));
        assert!(out.tooltip.contains("620 / 1000"));
        assert!(out.tooltip.contains("Pay-as-you-go"));
        assert!(out.tooltip.contains("This key"));
        assert!(out.tooltip.contains("Usage by endpoint"));
        assert!(out.tooltip.contains("search 350 · extract 75"));
    }

    #[test]
    fn unlimited_plan_tooltip_is_not_a_fake_percent() {
        let mut snap = sample_snap();
        snap.plan_limit = None;
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.tooltip.contains("unlimited"));
        assert!(!out.tooltip.contains("%"), "tooltip: {}", out.tooltip);
    }

    #[test]
    fn stale_appends_pause() {
        let snap = sample_snap();
        let mut outcome = sample_outcome(snap.clone());
        outcome.stale = true;
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.text.contains("⏸"));
    }

    #[test]
    fn schema_error_has_schema_label_not_http() {
        let snap = sample_snap();
        let mut outcome = sample_outcome(snap.clone());
        outcome.stale = true;
        outcome.last_error = Some((0, SCHEMA_DRIFT_MESSAGE.into()));
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.tooltip.contains("Tavily API schema drift"));
        assert!(!out.tooltip.contains("HTTP"));
    }

    #[test]
    fn fetch_outcome_conversion_preserves_metadata() {
        let snap = sample_snap();
        let fetch = FetchOutcome {
            snapshot: snap.clone(),
            stale: true,
            last_error: Some((401, "bad".into())),
            cache_age: Some(std::time::Duration::from_secs(42)),
        };
        let vendor: VendorOutcome = fetch.into();
        assert!(matches!(
            vendor.snapshot,
            crate::usage::VendorSnapshot::Tavily(_)
        ));
        assert!(vendor.stale);
        assert_eq!(vendor.last_error, Some((401, "bad".into())));
    }
}
