//! `ai-usagebar usage` — quota and time-to-reset for everything in the config,
//! in one pass.
//!
//! The widget answers "how is *this* vendor doing" one process at a time, which
//! is what a status bar needs and what a person checking on four Claude
//! accounts does not. This walks the same tab set the TUI builds — every
//! enabled vendor, plus one entry per named Claude account — and prints what
//! each one has left.
//!
//! Deliberately thin: [`crate::tui::app::tabs_from_config`] already decides
//! what is configured, [`crate::tui::app::refresh_one`] already fetches and
//! parses it, and [`crate::tui::panels::sections_for`] already projects any
//! vendor's snapshot into labelled sections carrying every reported value.
//! So this file only enumerates, projects, and formats — no vendor
//! ever needs to know it exists.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;

use crate::config::Config;
use crate::tui::app::{TabId, TabSource, TabState, refresh_one, tabs_with_desktop};
use crate::tui::panels::{Section, sections_with_metadata_for};

/// Matches the widget's `--pace-tolerance` default; only affects the pacing
/// note appended to a metric's detail line.
const PACE_TOLERANCE: u32 = 5;

/// Version of the tolerant, machine-readable `usage --json` contract.
/// Increment only when an incompatible change cannot be represented by adding
/// or omitting fields.
const USAGE_SCHEMA_VERSION: u8 = 1;

/// One configured vendor or account.
struct Entry {
    id: String,
    name: String,
    display_name: String,
    /// The vendor's `{vendor_short}` code. Frontends that want a Waybar-style
    /// provider tag take it from here rather than keeping their own table.
    short_name: String,
    /// The vendor's bar glyph. Same rule as `short_name`: one table, in Rust.
    icon: String,
    /// Built-in vendor slug whose brand mark this entry should be drawn with.
    /// A built-in is its own brand; a `[[custom]]` provider has one only when
    /// it declared `brand`. The asset itself stays with the frontend — each
    /// ships its own artwork — so this names the provider, never a file.
    brand: Option<String>,
    plan: Option<String>,
    sections: Vec<ReportSection>,
    error: Option<String>,
    stale: bool,
    fetched_at: Option<DateTime<Utc>>,
    /// Structured banked-reset inventory for rich frontends. The human-readable
    /// block remains in `sections` for the text report and older consumers.
    reset_credits: Option<crate::usage::ResetCredits>,
}

/// Lossless machine-readable projection of a TUI panel row. `metrics` remains
/// available in JSON as a convenience view over only the gauge rows; callers
/// that need every reported value should consume this ordered list.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ReportSection {
    Metric {
        label: String,
        percent: u16,
        value: String,
        detail: String,
        /// Which of `percent` and `value` this metric puts on the bar —
        /// `"percent"` or `"value"`. A consumer that honours it draws the named
        /// one and leaves the other in the detail line, rather than inferring a
        /// balance row from its label. Always present, so no consumer has to
        /// guess; not every consumer reads it — Waybar and GNOME take their bar
        /// text from per-vendor formats instead.
        headline: String,
        severity: String,
        reset_at: Option<DateTime<Utc>>,
        /// Full length of the reset window in seconds, present only when the
        /// vendor states it exactly (rolling 5h/7d windows). A frontend that
        /// wants a pace indicator needs both this and `reset_at`; a calendar
        /// month or an unstated window omits the field rather than guessing.
        #[serde(skip_serializing_if = "Option::is_none")]
        window_secs: Option<u64>,
        /// Sub-group heading the metric belongs under (SuperGrok's product
        /// slices under `"Breakdown"`), so a frontend can draw it compactly
        /// beneath that heading instead of as a peer of the overall meter.
        /// Absent for metrics that stand on their own.
        #[serde(skip_serializing_if = "Option::is_none")]
        group: Option<String>,
    },
    Text {
        label: String,
        value: String,
    },
    Block {
        label: String,
        body: Vec<String>,
    },
    Spacer,
}

impl ReportSection {
    fn label(&self) -> Option<&str> {
        match self {
            Self::Metric { label, .. } | Self::Text { label, .. } | Self::Block { label, .. } => {
                Some(label)
            }
            Self::Spacer => None,
        }
    }
}

/// Snapshot every configured vendor as the JSON `usage --json` prints.
///
/// A frontend hosted in the same process calls this instead of spawning a
/// console subprocess. Errors are the same user-facing strings `usage` would
/// print.
pub async fn collect_json() -> std::result::Result<String, String> {
    let (entries, primary) = collect_entries().await?;
    Ok(render_json_for_primary(&entries, primary))
}

/// Snapshot one configured vendor or account — the entry whose `id` is
/// `entry_id`, as `collect_json` would have labelled it — as `{"entries": [..]}`.
///
/// A frontend's per-provider "Refresh" calls this so re-fetching one vendor
/// does not hit every other one. No `primary` key: the caller already knows
/// which entry it asked for.
pub async fn collect_entry_json(entry_id: &str) -> std::result::Result<String, String> {
    let config = Config::load().map_err(|error| error.user_message())?;
    let client = crate::widget::run::http_client().map_err(|error| error.user_message())?;
    let tabs = tabs_matching(&tabs_with_desktop(&config), entry_id);
    if tabs.is_empty() {
        return Err(format!("no enabled provider matches {entry_id}"));
    }
    let entries = collect_entries_for(&client, &config, &tabs).await;
    Ok(render_json_entries(&entries))
}

/// The tabs whose report entry would carry `entry_id` — at most one, since
/// [`tab_id`] is unique across a tab list, but kept as a slice so the caller
/// runs the same loop as the full report.
fn tabs_matching(tabs: &[TabId], entry_id: &str) -> Vec<TabId> {
    tabs.iter()
        .filter(|tab| tab_id(tab) == entry_id)
        .cloned()
        .collect()
}

async fn collect_entries() -> std::result::Result<(Vec<Entry>, Option<&'static str>), String> {
    let config = Config::load().map_err(|error| error.user_message())?;
    let client = crate::widget::run::http_client().map_err(|error| error.user_message())?;
    let tabs = tabs_with_desktop(&config);
    if tabs.is_empty() {
        return Err(format!(
            "no vendors enabled in {}",
            crate::config::config_path_hint()
        ));
    }
    let entries = collect_entries_for(&client, &config, &tabs).await;
    Ok((entries, config.ui.primary.map(|vendor| vendor.slug())))
}

async fn collect_entries_for(
    client: &reqwest::Client,
    config: &Config,
    tabs: &[TabId],
) -> Vec<Entry> {
    // Sequential on purpose: several of these share a per-vendor cache lock,
    // and firing every account at Anthropic at once is a good way to get
    // rate-limited for no gain on a handful of entries.
    let mut entries = Vec::with_capacity(tabs.len());
    for tab in tabs {
        entries.push(entry_for(client, config, tab).await);
    }
    entries
}

pub async fn run(json: bool) -> i32 {
    let (entries, primary) = match collect_entries().await {
        Ok(pair) => pair,
        Err(message) => {
            eprintln!("ai-usagebar usage: {message}");
            return 1;
        }
    };

    if json {
        println!("{}", render_json_for_primary(&entries, primary));
    } else {
        print!("{}", render_text(&entries));
    }
    report_exit_code(&entries)
}

async fn entry_for(client: &reqwest::Client, config: &Config, tab: &TabId) -> Entry {
    let state = refresh_one(client, config, tab).await;
    entry_from_state_with_config(config, tab, &state, Utc::now())
}

fn entry_from_state_with_config(
    config: &Config,
    tab: &TabId,
    state: &TabState,
    now: chrono::DateTime<Utc>,
) -> Entry {
    let mut entry = entry_from_state(tab, state, now);
    if let TabSource::Custom { id, .. } = &tab.source {
        entry.brand = config
            .custom_by_id(id)
            .and_then(|provider| provider.brand.clone());
    }
    entry
}

fn entry_from_state(tab: &TabId, state: &TabState, now: chrono::DateTime<Utc>) -> Entry {
    let mut entry = Entry {
        id: tab_id(tab),
        name: tab_name(tab),
        display_name: tab_display_name(tab),
        // #164's naming (a custom tab has no VendorId) with #162's plan
        // (an error card still names the plan from the OAuth blob).
        short_name: tab_short_name(tab),
        icon: tab_icon(tab),
        brand: tab_brand(tab),
        plan: match state {
            TabState::Error { plan, .. } => plan
                .as_deref()
                .map(crate::display::sanitize_untrusted_field)
                .filter(|plan| !plan.is_empty()),
            _ => None,
        },
        sections: Vec::new(),
        error: match state {
            TabState::Error { message, .. } => {
                Some(crate::display::sanitize_untrusted_field(message))
            }
            _ => None,
        },
        stale: matches!(state, TabState::Ready(ready) if ready.stale),
        fetched_at: match state {
            TabState::Ready(ready) => ready.fetched_at,
            _ => None,
        },
        reset_credits: reset_credits_for(state),
    };
    // The error is already a first-class entry field. Do not duplicate the
    // TUI's interactive retry instructions as report data.
    if entry.error.is_some() {
        return entry;
    }
    for projected in sections_with_metadata_for(state, now, PACE_TOLERANCE) {
        match projected.section {
            Section::Title { left, .. } => entry.plan = Some(left),
            Section::Metric {
                label,
                pct,
                value_label,
                footnote,
                severity,
                ..
            } => {
                entry.sections.push(ReportSection::Metric {
                    label,
                    percent: pct,
                    value: value_label,
                    detail: footnote,
                    headline: projected.headline.as_str().into(),
                    severity: severity.as_str().into(),
                    reset_at: projected.reset_at,
                    window_secs: projected
                        .window
                        .map(|window| window.num_seconds().max(0) as u64),
                    group: projected.group.map(str::to_string),
                });
            }
            Section::Text { label, value } => {
                entry.sections.push(ReportSection::Text { label, value });
            }
            Section::Block { label, body } => {
                entry.sections.push(ReportSection::Block { label, body });
            }
            Section::Spacer => entry.sections.push(ReportSection::Spacer),
        }
    }
    entry
}

fn reset_credits_for(state: &TabState) -> Option<crate::usage::ResetCredits> {
    let credits = match state {
        TabState::Ready(ready) => ready.snapshot.reset_credits()?,
        _ => return None,
    };
    (!credits.is_empty()).then(|| credits.clone())
}

/// Process status after a complete document has been printed.
///
/// Per-entry fetch/auth failures are data inside the document, not a command
/// failure. Empty is not a document — [`collect_entries`] already fails before
/// this when nothing is enabled.
fn report_exit_code(entries: &[Entry]) -> i32 {
    i32::from(entries.is_empty())
}

/// Stable machine id shared by aggregate views and the macOS menu bar:
/// `<vendor>@<label>` for named accounts, `custom:<id>` for a `[[custom]]`
/// provider (which never has accounts). Also the entry half of the
/// notification dedupe key.
pub(crate) fn tab_id(tab: &TabId) -> String {
    match &tab.source {
        TabSource::Custom { id, .. } => format!("custom:{id}"),
        TabSource::Builtin(vendor) => match &tab.account {
            Some(account) => format!("{}@{account}", vendor.slug()),
            None => vendor.slug().to_string(),
        },
    }
}

fn tab_name(tab: &TabId) -> String {
    match &tab.source {
        TabSource::Builtin(vendor) => format_tab_name(tab, vendor.slug()),
        TabSource::Custom { name, .. } => format_tab_name(tab, name),
    }
}

fn tab_display_name(tab: &TabId) -> String {
    match &tab.source {
        TabSource::Builtin(vendor) => format_tab_name(tab, vendor.display_name()),
        TabSource::Custom { name, .. } => format_tab_name(tab, name),
    }
}

/// The `{vendor_short}` code: the vendor's own for a built-in, the configured
/// `short_name` for a custom provider.
fn tab_short_name(tab: &TabId) -> String {
    match &tab.source {
        TabSource::Builtin(vendor) => vendor.short_name().to_string(),
        TabSource::Custom { short_name, .. } => {
            crate::display::sanitize_untrusted_field(short_name)
        }
    }
}

/// The bar glyph: the vendor's own for a built-in. A custom provider has no
/// glyph of its own, so its `short_name` stands in, as Zai and Kimi's do.
fn tab_icon(tab: &TabId) -> String {
    match &tab.source {
        TabSource::Builtin(vendor) => vendor.bar_icon().to_string(),
        TabSource::Custom { short_name, .. } => {
            crate::display::sanitize_untrusted_field(short_name)
        }
    }
}

/// The provider whose mark draws this entry. Built-in tabs carry everything
/// needed to derive it; a custom tab's optional brand stays in `Config` rather
/// than widening the public `TabSource` enum and is attached by
/// `entry_from_state_with_config`.
fn tab_brand(tab: &TabId) -> Option<String> {
    match &tab.source {
        TabSource::Builtin(vendor) => Some(vendor.slug().to_string()),
        TabSource::Custom { .. } => None,
    }
}

fn format_tab_name(tab: &TabId, vendor_name: &str) -> String {
    let name = match &tab.account {
        // Mark a Desktop-sourced account so a mixed CLI+Desktop setup is legible;
        // for a Desktop-only user every Claude row simply reads "· <label> (desktop)".
        Some(account) if tab.desktop => format!("{vendor_name} · {account} (desktop)"),
        Some(account) => format!("{vendor_name} · {account}"),
        None => vendor_name.to_string(),
    };
    crate::display::sanitize_untrusted_field(&name)
}

/// Resolve the `primary` the report should name to an id `entries` actually
/// carries.
///
/// `config.ui.primary` is a vendor slug, but with named accounts the entry
/// ids are `vendor@account`, so the raw slug names an id no entry has —
/// `primary: "anthropic"` next to `anthropic@claude-me`. The first entry of
/// that vendor (the bare slug, or the first `{slug}@…` account) wins; a slug
/// with no matching entry is kept as-is, because the config naming a
/// disabled or absent vendor is information worth reporting, not something
/// to paper over with a guess. `None` (unset) stays `None`.
fn resolve_primary(primary: Option<&str>, entries: &[Entry]) -> Option<String> {
    primary.map(|slug| {
        // The `@` suffix keeps a slug that is a prefix of another vendor's
        // ("openai" vs "openrouter") from matching that vendor's accounts.
        entries
            .iter()
            .find(|entry| entry.id == slug || entry.id.starts_with(&format!("{slug}@")))
            .map(|entry| entry.id.clone())
            .unwrap_or_else(|| slug.to_string())
    })
}

fn render_json_for_primary(entries: &[Entry], primary: Option<&str>) -> String {
    json!({
        "schema_version": USAGE_SCHEMA_VERSION,
        "primary": resolve_primary(primary, entries),
        "entries": json_rows(entries),
    })
    .to_string()
}

/// The single-entry shape: the same rows, without a `primary` the caller
/// did not ask about.
fn render_json_entries(entries: &[Entry]) -> String {
    json!({
        "schema_version": USAGE_SCHEMA_VERSION,
        "entries": json_rows(entries),
    })
    .to_string()
}

fn json_rows(entries: &[Entry]) -> Vec<serde_json::Value> {
    entries
        .iter()
        .map(|entry| {
            let metrics = entry
                .sections
                .iter()
                .filter_map(|section| match section {
                    ReportSection::Metric {
                        label,
                        percent,
                        value,
                        detail,
                        headline,
                        severity,
                        reset_at,
                        window_secs,
                        group,
                    } => {
                        let mut metric = json!({
                            "label": label,
                            "percent": percent,
                            "value": value,
                            "detail": detail,
                            "headline": headline,
                            "severity": severity,
                            "reset_at": reset_at,
                        });
                        // Same rule as the `sections` serializer: absent, not null.
                        if let Some(secs) = window_secs {
                            metric["window_secs"] = json!(secs);
                        }
                        if let Some(group) = group {
                            metric["group"] = json!(group);
                        }
                        Some(metric)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let mut row = json!({
                "id": entry.id,
                "name": entry.name,
                "display_name": entry.display_name,
                "short_name": entry.short_name,
                "icon": entry.icon,
                "plan": entry.plan,
                "status": if entry.error.is_some() { "error" } else { "ready" },
                "error": entry.error,
                "stale": entry.stale,
                "fetched_at": entry.fetched_at,
                "reset_credits": entry.reset_credits,
                "metrics": metrics,
                "sections": entry.sections,
            });
            // Optional additive fields are absent, not null, so older and
            // newer producers keep the documented tolerant JSON contract.
            if let Some(brand) = &entry.brand {
                row["brand"] = json!(brand);
            }
            row
        })
        .collect()
}

fn render_text(entries: &[Entry]) -> String {
    // Widest label across every entry, so the value column lines up down the
    // whole report rather than per-section.
    let width = entries
        .iter()
        .flat_map(|entry| entry.sections.iter())
        .filter_map(ReportSection::label)
        .map(crate::display::text_width)
        .max()
        .unwrap_or(0);

    let mut out = String::new();
    for entry in entries {
        out.push_str(&entry.name);
        if let Some(plan) = &entry.plan {
            out.push_str(&format!("   {plan}"));
        }
        out.push('\n');
        if let Some(error) = &entry.error {
            out.push_str(&format!("  ! {error}\n\n"));
            continue;
        }
        if !entry
            .sections
            .iter()
            .any(|section| !matches!(section, ReportSection::Spacer))
        {
            out.push_str("  (nothing reported)\n\n");
            continue;
        }
        let mut body = String::new();
        let mut pending_spacer = false;
        for section in &entry.sections {
            if matches!(section, ReportSection::Spacer) {
                pending_spacer |= !body.is_empty();
                continue;
            }
            if pending_spacer {
                body.push('\n');
                pending_spacer = false;
            }
            match section {
                ReportSection::Metric {
                    label,
                    value,
                    detail,
                    ..
                } => {
                    let label = crate::display::pad_end(label, width);
                    let value = format!("{value:>9}");
                    if detail.is_empty() {
                        body.push_str(&format!("  {label}  {value}\n"));
                    } else {
                        body.push_str(&format!("  {label}  {value}   {detail}\n"));
                    }
                }
                ReportSection::Text { label, value } => {
                    if label.is_empty() {
                        body.push_str(&format!("  {}\n", value.trim_start()));
                    } else if value.is_empty() {
                        body.push_str(&format!("  {label}\n"));
                    } else {
                        let label = crate::display::pad_end(label, width);
                        body.push_str(&format!("  {label}  {value}\n"));
                    }
                }
                ReportSection::Block { label, body: lines } => {
                    body.push_str(&format!("  {label}\n"));
                    for line in lines {
                        body.push_str(&format!("    {line}\n"));
                    }
                }
                ReportSection::Spacer => unreachable!(),
            }
        }
        out.push_str(&body);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::ReadyTab;
    use crate::usage::{
        DeepseekSnapshot, KimiSnapshot, KiroSnapshot, OpenAiSnapshot, OpenAiSource,
        OpenRouterSnapshot, ResetCredit, ResetCredits, SuperGrokPeriod, SuperGrokSnapshot,
        VendorSnapshot,
    };
    use crate::vendor::VendorId;

    fn entry(name: &str, sections: Vec<ReportSection>) -> Entry {
        Entry {
            id: name.into(),
            name: name.into(),
            display_name: name.into(),
            short_name: VendorId::Anthropic.short_name().into(),
            icon: VendorId::Anthropic.bar_icon().into(),
            brand: Some(VendorId::Anthropic.slug().into()),
            plan: Some("Claude Max 20x".into()),
            sections,
            error: None,
            stale: false,
            fetched_at: None,
            reset_credits: None,
        }
    }

    fn metric(label: &str, percent: u16, value: &str, detail: &str) -> ReportSection {
        ReportSection::Metric {
            label: label.into(),
            percent,
            value: value.into(),
            detail: detail.into(),
            headline: "percent".into(),
            severity: "mid".into(),
            reset_at: None,
            window_secs: None,
            group: None,
        }
    }

    #[test]
    fn accounts_get_a_stable_id_and_a_readable_name() {
        let account = TabId::account("gmail");
        assert_eq!(tab_id(&account), "anthropic@gmail");
        assert_eq!(tab_name(&account), "anthropic · gmail");
        assert_eq!(tab_display_name(&account), "Claude · gmail");

        let plain = TabId::vendor(VendorId::Cursor);
        assert_eq!(tab_id(&plain), "cursor");
        assert_eq!(tab_name(&plain), "cursor");
        assert_eq!(tab_display_name(&plain), "Cursor");

        let openrouter = TabId::account_for(VendorId::Openrouter, "work");
        assert_eq!(tab_id(&openrouter), "openrouter@work");
        assert_eq!(tab_name(&openrouter), "openrouter · work");
        assert_eq!(tab_display_name(&openrouter), "OpenRouter · work");
    }

    /// The Omarchy bar can be set to draw the provider tag and nothing else,
    /// so every entry — a failed one included — has to carry a code, and every
    /// account of one vendor has to carry that vendor's code rather than a
    /// per-account one.
    #[test]
    fn every_entry_carries_its_vendor_short_code() {
        let now = Utc::now();
        let failed = TabState::error("not signed in");

        let account = entry_from_state(&TabId::account("gmail"), &failed, now);
        assert_eq!(account.short_name, "cld");
        let other_account = entry_from_state(&TabId::account("work"), &failed, now);
        assert_eq!(other_account.short_name, account.short_name);

        let cursor = entry_from_state(&TabId::vendor(VendorId::Cursor), &failed, now);
        assert_eq!(cursor.short_name, "cur");
        assert_eq!(cursor.icon, VendorId::Cursor.bar_icon());
        // A built-in vendor is its own brand, so a frontend never has to map
        // the entry id back to a provider to pick the mark.
        assert_eq!(cursor.brand.as_deref(), Some(VendorId::Cursor.slug()));

        let rendered = render_json_for_primary(&[cursor], None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["entries"][0]["short_name"], "cur");
        assert_eq!(value["entries"][0]["icon"], VendorId::Cursor.bar_icon());
        assert_eq!(value["entries"][0]["brand"], VendorId::Cursor.slug());
    }

    #[test]
    fn every_metric_reports_its_quota_and_its_reset() {
        let text = render_text(&[entry(
            "anthropic · gmail",
            vec![
                metric("Session (5h)", 29, "29%", "Resets in 0h 50m"),
                metric("Weekly (7d)", 32, "32%", "Resets in 4d 2h"),
            ],
        )]);

        assert!(
            text.contains("anthropic · gmail   Claude Max 20x"),
            "{text}"
        );
        assert!(text.contains("29%   Resets in 0h 50m"), "{text}");
        assert!(text.contains("32%   Resets in 4d 2h"), "{text}");
    }

    /// Labels are padded to one width across the whole report, so the columns
    /// still line up when a later entry has a longer label than the first.
    #[test]
    fn value_columns_align_across_entries() {
        let text = render_text(&[
            entry("a", vec![metric("S", 1, "1%", "")]),
            entry("b", vec![metric("A very long label", 2, "2%", "")]),
        ]);
        let columns: Vec<usize> = text
            .lines()
            .filter(|line| line.starts_with("  ") && line.contains('%'))
            .map(|line| line.find('%').unwrap())
            .collect();
        assert_eq!(columns.len(), 2);
        assert_eq!(columns[0], columns[1], "{text}");
    }

    /// The same alignment, but with a label whose glyphs are two columns wide.
    /// `format!("{label:width$}")` pads by character count, so a CJK label used
    /// to leave the value column short by one space per ideograph. Note the
    /// column is measured in display width, not byte or char offset — `find`
    /// returns a byte index, which is itself three per ideograph here.
    #[test]
    fn value_columns_align_when_a_label_is_double_width() {
        let text = render_text(&[
            entry("a", vec![metric("セッション", 1, "1%", "")]),
            entry("b", vec![metric("Weekly", 2, "2%", "")]),
        ]);
        let columns: Vec<usize> = text
            .lines()
            .filter(|line| line.starts_with("  ") && line.contains('%'))
            .map(|line| {
                let byte = line.find('%').unwrap();
                crate::display::text_width(&line[..byte])
            })
            .collect();
        assert_eq!(columns.len(), 2);
        assert_eq!(columns[0], columns[1], "{text}");
    }

    /// One dead vendor must not hide the others — it reports inline and the
    /// rest still print.
    #[test]
    fn a_failing_entry_is_reported_without_dropping_the_rest() {
        let mut broken = entry("openai", Vec::new());
        broken.error = Some("credentials error: not signed in".into());
        let text = render_text(&[broken, entry("cursor", vec![metric("Auto", 5, "5%", "")])]);

        assert!(
            text.contains("! credentials error: not signed in"),
            "{text}"
        );
        assert!(text.contains("cursor"), "{text}");
        assert!(text.contains("5%"), "{text}");
    }

    #[test]
    fn json_carries_the_percentage_as_a_number() {
        let rendered = render_json_for_primary(
            &[entry(
                "anthropic · gmail",
                vec![metric("Session (5h)", 29, "29%", "Resets in 0h 50m")],
            )],
            None,
        );
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let first = &value["entries"][0];
        assert_eq!(first["plan"], "Claude Max 20x");
        assert_eq!(first["display_name"], "anthropic · gmail");
        assert_eq!(first["short_name"], "cld");
        assert_eq!(first["metrics"][0]["percent"], 29);
        assert_eq!(first["metrics"][0]["detail"], "Resets in 0h 50m");
        assert!(first["error"].is_null());
        assert_eq!(first["status"], "ready");
        assert_eq!(first["stale"], false);
        assert!(first["fetched_at"].is_null());
        assert_eq!(first["metrics"][0]["severity"], "mid");
        assert!(first["metrics"][0]["reset_at"].is_null());
        assert!(value["primary"].is_null());
    }

    #[test]
    fn json_carries_the_configured_primary_without_reordering_entries() {
        let rendered = render_json_for_primary(
            &[entry("anthropic", Vec::new()), entry("openai", Vec::new())],
            Some("openai"),
        );
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["primary"], "openai");
        assert_eq!(value["entries"][0]["id"], "anthropic");
        assert_eq!(value["entries"][1]["id"], "openai");
    }

    /// #228: `config.ui.primary` is a vendor slug, but entry ids carry
    /// account labels for named accounts, so the reported `primary` is
    /// resolved to an id one of the entries actually has.
    #[test]
    fn primary_resolves_to_an_entry_id_the_report_actually_carries() {
        // Bare entries: the slug already is an entry id, so it stays.
        let bare = vec![entry("anthropic", Vec::new()), entry("openai", Vec::new())];
        assert_eq!(
            resolve_primary(Some("anthropic"), &bare),
            Some("anthropic".into())
        );

        // Named accounts only: the first account's entry id is reported, so
        // `primary` names an id `entries` carries.
        let accounts = vec![
            entry("anthropic@claude-me", Vec::new()),
            entry("anthropic@claude-b3", Vec::new()),
        ];
        assert_eq!(
            resolve_primary(Some("anthropic"), &accounts),
            Some("anthropic@claude-me".into())
        );

        // A bare entry wins over accounts of the same vendor: it is the
        // first entry the slug matches.
        let mixed = vec![
            entry("anthropic", Vec::new()),
            entry("anthropic@claude-me", Vec::new()),
        ];
        assert_eq!(
            resolve_primary(Some("anthropic"), &mixed),
            Some("anthropic".into())
        );

        // A slug that prefixes another vendor's name must not match that
        // vendor's entries: the `@` delimiter is what keeps this exact.
        let openrouter = vec![entry("openrouter@work", Vec::new())];
        assert_eq!(
            resolve_primary(Some("openai"), &openrouter),
            Some("openai".into())
        );

        // No matching entry: the config names a disabled or absent vendor,
        // which is worth reporting as-is rather than papering over.
        assert_eq!(
            resolve_primary(Some("cursor"), &accounts),
            Some("cursor".into())
        );

        // Unset stays unset.
        assert_eq!(resolve_primary(None, &accounts), None);
    }

    /// The same resolution as seen through the rendered JSON: consumers may
    /// now treat `primary` as an entry id present in `entries`.
    #[test]
    fn json_primary_resolves_to_the_first_named_account_entry() {
        let rendered = render_json_for_primary(
            &[
                entry("anthropic@claude-me", Vec::new()),
                entry("anthropic@claude-b3", Vec::new()),
            ],
            Some("anthropic"),
        );
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["primary"], "anthropic@claude-me");
        let ids: Vec<&str> = value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&value["primary"].as_str().unwrap()));

        // The honest fallback: a primary naming a vendor with no entries
        // keeps the slug instead of picking some other entry.
        let unmatched =
            render_json_for_primary(&[entry("anthropic@claude-me", Vec::new())], Some("cursor"));
        let value: serde_json::Value = serde_json::from_str(&unmatched).unwrap();
        assert_eq!(value["primary"], "cursor");
        assert_eq!(value["entries"][0]["id"], "anthropic@claude-me");
    }

    #[test]
    fn every_json_report_declares_its_schema_version() {
        let aggregate: serde_json::Value = serde_json::from_str(&render_json_for_primary(
            &[entry("anthropic", Vec::new())],
            Some("anthropic"),
        ))
        .unwrap();
        assert_eq!(aggregate["schema_version"], 1);

        let single: serde_json::Value =
            serde_json::from_str(&render_json_entries(&[entry("anthropic", Vec::new())])).unwrap();
        assert_eq!(single["schema_version"], 1);
    }

    #[test]
    fn json_exposes_absolute_resets_and_cache_freshness_additively() {
        let fetched_at = Utc::now() - chrono::Duration::minutes(3);
        let reset_at = Utc::now() + chrono::Duration::days(1);
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Kiro(KiroSnapshot {
                plan: "KIRO POWER".into(),
                used: 4_000.0,
                limit: 10_000.0,
                reset_at: Some(reset_at),
            }),
            stale: true,
            last_error: None,
            fetched_at: Some(fetched_at),
            display: Default::default(),
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Kiro), &state, Utc::now());
        let rendered = render_json_for_primary(&[projected], None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let first = &value["entries"][0];

        assert_eq!(first["stale"], true);
        let fetched_rfc3339 = fetched_at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true);
        let reset_rfc3339 = reset_at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true);
        assert_eq!(first["fetched_at"], fetched_rfc3339);
        assert_eq!(first["metrics"][0]["reset_at"], reset_rfc3339);
        assert_eq!(first["sections"][1]["reset_at"], reset_rfc3339);
        assert_eq!(first["metrics"][0]["severity"], "low");
        // Kiro states a reset but not how long its window is; a frontend
        // must not be handed a length to pace against.
        assert!(first["metrics"][0]["window_secs"].is_null());
        assert!(first["sections"][1].get("window_secs").is_none());
    }

    /// Grouped sub-rows (SuperGrok's product slices) carry their group in both
    /// the ordered `sections` and the `metrics` convenience view, and the field
    /// is omitted (not `null`) for metrics that stand on their own — including
    /// the overall meter they break down.
    #[test]
    fn json_carries_the_group_only_for_grouped_slices() {
        use crate::usage::{ResetCredits, SuperGrokPeriod, SuperGrokProduct, SuperGrokSnapshot};

        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::SuperGrok(SuperGrokSnapshot {
                plan: "SuperGrok Heavy".into(),
                account: "scope".into(),
                weekly_pct: 97,
                period: SuperGrokPeriod::Weekly,
                reset_at: Some(Utc::now() + chrono::Duration::days(3)),
                prepaid_balance: None,
                reset_credits: ResetCredits::default(),
                products: vec![SuperGrokProduct {
                    label: "Grok Build".into(),
                    percent: 94,
                }],
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
            display: Default::default(),
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Supergrok), &state, Utc::now());
        let rendered = render_json_for_primary(&[projected], None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let first = &value["entries"][0];

        // sections: [spacer, overall, product slice] — the overall meter the
        // slices break down carries no group; the slice does, in both views.
        assert!(first["sections"][1].get("group").is_none());
        assert_eq!(first["sections"][2]["group"], "Breakdown");
        assert!(first["metrics"][0].get("group").is_none());
        assert_eq!(first["metrics"][1]["group"], "Breakdown");
    }

    #[test]
    fn json_exposes_banked_reset_expiries_without_removing_the_text_block() {
        let expiry: DateTime<Utc> = "2026-09-20T23:58:00Z".parse().unwrap();
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Openai(OpenAiSnapshot {
                plan: "ChatGPT Pro".into(),
                session: None,
                weekly: None,
                code_review: None,
                additional_limits: Vec::new(),
                unavailable_models: Vec::new(),
                credits: None,
                reset_credits: ResetCredits {
                    available: 2,
                    credits: vec![ResetCredit {
                        title: Some("Full reset".into()),
                        expires_at: Some(expiry),
                    }],
                },
                source: OpenAiSource::CodexOauth,
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
            display: Default::default(),
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Openai), &state, Utc::now());
        let value: serde_json::Value =
            serde_json::from_str(&render_json_for_primary(&[projected], None)).unwrap();
        let entry = &value["entries"][0];

        assert_eq!(entry["reset_credits"]["available"], 2);
        assert_eq!(entry["reset_credits"]["credits"][0]["title"], "Full reset");
        assert_eq!(
            entry["reset_credits"]["credits"][0]["expires_at"],
            expiry.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
        );
        assert!(
            entry["sections"].as_array().unwrap().iter().any(|section| {
                section["type"] == "block" && section["label"] == "Reset credits"
            })
        );
    }

    /// The sidebar reads `reset_credits` off the entry, so Claude's grant has
    /// to arrive through the same field Codex and SuperGrok already use —
    /// not a Claude-shaped one a frontend would have to learn.
    #[test]
    fn json_exposes_claude_reset_credit_expiries() {
        let expiry: DateTime<Utc> = "2026-10-22T16:00:00Z".parse().unwrap();
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Anthropic(crate::usage::AnthropicSnapshot {
                plan: "Max 20x".into(),
                session: crate::usage::UsageWindow {
                    utilization_pct: 2,
                    resets_at: None,
                    window_duration: chrono::Duration::hours(5),
                },
                weekly: crate::usage::UsageWindow {
                    utilization_pct: 63,
                    resets_at: None,
                    window_duration: chrono::Duration::days(7),
                },
                sonnet: None,
                scoped: Vec::new(),
                extra: None,
                reset_credits: ResetCredits {
                    available: 1,
                    credits: vec![ResetCredit {
                        title: Some("Opus 5.5 launch reset".into()),
                        expires_at: Some(expiry),
                    }],
                },
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
            display: Default::default(),
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Anthropic), &state, Utc::now());
        let value: serde_json::Value =
            serde_json::from_str(&render_json_for_primary(&[projected], None)).unwrap();
        let entry = &value["entries"][0];

        assert_eq!(entry["reset_credits"]["available"], 1);
        assert_eq!(
            entry["reset_credits"]["credits"][0]["title"],
            "Opus 5.5 launch reset"
        );
        assert_eq!(
            entry["reset_credits"]["credits"][0]["expires_at"],
            expiry.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
        );
        assert!(
            entry["sections"].as_array().unwrap().iter().any(|section| {
                section["type"] == "block" && section["label"] == "Reset credits"
            })
        );
    }

    #[test]
    fn json_exposes_supergrok_reset_credit_expiries() {
        let expiry: DateTime<Utc> = "2026-10-03T23:00:00Z".parse().unwrap();
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::SuperGrok(SuperGrokSnapshot {
                plan: "SuperGrok".into(),
                account: "test-account".into(),
                weekly_pct: 0,
                period: SuperGrokPeriod::Weekly,
                reset_at: None,
                prepaid_balance: None,
                reset_credits: ResetCredits {
                    available: 1,
                    credits: vec![ResetCredit {
                        title: None,
                        expires_at: Some(expiry),
                    }],
                },
                products: Vec::new(),
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
            display: Default::default(),
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Supergrok), &state, Utc::now());
        let value: serde_json::Value =
            serde_json::from_str(&render_json_for_primary(&[projected], None)).unwrap();
        let entry = &value["entries"][0];

        assert_eq!(entry["reset_credits"]["available"], 1);
        assert_eq!(
            entry["reset_credits"]["credits"][0]["expires_at"],
            expiry.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
        );
    }

    /// A rolling window's exact length rides along with its row, in both the
    /// ordered `sections` and the `metrics` convenience view, and is omitted
    /// (not `null`) for a metric that has none.
    #[test]
    fn json_carries_the_window_length_only_for_exact_windows() {
        use crate::usage::{AnthropicSnapshot, UsageWindow};

        let now = Utc::now();
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Anthropic(AnthropicSnapshot {
                plan: "Claude Max 20x".into(),
                session: UsageWindow {
                    utilization_pct: 29,
                    resets_at: Some(now + chrono::Duration::minutes(50)),
                    window_duration: chrono::Duration::hours(5),
                },
                weekly: UsageWindow {
                    utilization_pct: 32,
                    resets_at: Some(now + chrono::Duration::days(4)),
                    window_duration: chrono::Duration::days(7),
                },
                sonnet: None,
                scoped: vec![],
                extra: None,
                reset_credits: Default::default(),
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
            display: Default::default(),
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Anthropic), &state, now);
        let rendered = render_json_for_primary(&[projected], None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let first = &value["entries"][0];

        let window_of = |label: &str| {
            first["sections"]
                .as_array()
                .unwrap()
                .iter()
                .find(|section| section["label"] == label)
                .map(|section| section["window_secs"].clone())
                .unwrap_or_else(|| panic!("no section labelled {label}"))
        };
        assert_eq!(window_of("Session (5h)"), 18_000);
        assert_eq!(window_of("Weekly (7d)"), 604_800);
        assert_eq!(first["metrics"][0]["label"], "Session (5h)");
        assert_eq!(first["metrics"][0]["window_secs"], 18_000);
        assert_eq!(first["metrics"][1]["window_secs"], 604_800);

        // Same shape from the single-entry collector a frontend refreshes one provider with.
        let single: serde_json::Value =
            serde_json::from_str(&render_json_entries(&[entry_from_state(
                &TabId::vendor(VendorId::Anthropic),
                &state,
                now,
            )]))
            .unwrap();
        assert!(single.get("primary").is_none());
        assert_eq!(single["entries"][0]["metrics"][0]["window_secs"], 18_000);

        // A hand-built metric with no window serializes without the key at all.
        let bare =
            render_json_for_primary(&[entry("cursor", vec![metric("Auto", 5, "5%", "")])], None);
        let bare: serde_json::Value = serde_json::from_str(&bare).unwrap();
        assert!(
            bare["entries"][0]["metrics"][0]
                .get("window_secs")
                .is_none()
        );
        assert!(
            bare["entries"][0]["sections"][0]
                .get("window_secs")
                .is_none()
        );
    }

    /// A per-provider refresh narrows the configured tab list to the
    /// one whose report id it was shown; anything else yields nothing rather
    /// than a fallback to the whole report.
    #[test]
    fn tabs_matching_selects_exactly_the_entry_with_that_id() {
        use crate::tui::app::tabs_from_config;

        // Flip the flags both ways rather than trusting any vendor's default,
        // so the match below is this test's doing and the miss is a real one.
        let mut config = Config::default();
        config.zai.enabled = false;
        config.deepseek.enabled = false;
        assert!(tabs_matching(&tabs_from_config(&config), "zai").is_empty());
        assert!(tabs_matching(&tabs_from_config(&config), "deepseek").is_empty());
        config.zai.enabled = true;
        config.deepseek.enabled = true;
        let tabs = tabs_from_config(&config);

        assert_eq!(
            tabs_matching(&tabs, "zai"),
            vec![TabId::vendor(VendorId::Zai)]
        );
        assert_eq!(
            tabs_matching(&tabs, "deepseek"),
            vec![TabId::vendor(VendorId::Deepseek)]
        );
        assert!(tabs_matching(&tabs, "not-a-vendor").is_empty());
        assert!(tabs_matching(&tabs, "zai@work").is_empty());
        assert!(tabs_matching(&tabs, "").is_empty());
    }

    /// Named accounts are addressed by the same `<vendor>@<label>` id the
    /// report prints, so a frontend can refresh one Claude account by itself.
    #[test]
    fn tabs_matching_addresses_named_accounts_by_report_id() {
        let tabs = vec![
            TabId::vendor(VendorId::Anthropic),
            TabId::account("gmail"),
            TabId::account("work"),
        ];
        assert_eq!(
            tabs_matching(&tabs, "anthropic@work"),
            vec![TabId::account("work")]
        );
        assert_eq!(
            tabs_matching(&tabs, "anthropic"),
            vec![TabId::vendor(VendorId::Anthropic)]
        );
        assert!(tabs_matching(&tabs, "anthropic@nobody").is_empty());
    }

    #[test]
    fn report_reset_metadata_follows_multi_metric_order() {
        let weekly_reset = Utc::now() + chrono::Duration::days(3);
        let window_reset = Utc::now() + chrono::Duration::hours(2);
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Kimi(KimiSnapshot {
                plan: Some("Kimi Code".into()),
                weekly_limit: 1_000,
                weekly_used: 200,
                weekly_remaining: 800,
                weekly_reset_at: Some(weekly_reset),
                has_weekly: true,
                monthly_pct: None,
                monthly_reset_at: None,
                window_limit: 100,
                window_used: 40,
                window_remaining: 60,
                window_reset_at: Some(window_reset),
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
            display: Default::default(),
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Kimi), &state, Utc::now());
        // Pair each reset with its own row rather than pinning the row order —
        // that order is `panels::kimi_sections`' to choose, and pinning it here
        // would only duplicate the test that owns it.
        let resets: Vec<_> = projected
            .sections
            .iter()
            .filter_map(|section| match section {
                ReportSection::Metric {
                    label, reset_at, ..
                } => Some((label.as_str(), *reset_at)),
                _ => None,
            })
            .collect();

        assert_eq!(
            resets
                .iter()
                .copied()
                .collect::<std::collections::HashMap<_, _>>(),
            std::collections::HashMap::from([
                ("Weekly quota", Some(weekly_reset)),
                ("Rolling window (5h)", Some(window_reset)),
            ])
        );
    }

    #[test]
    fn json_preserves_non_metric_sections_without_fabricating_percentages() {
        let rendered = render_json_for_primary(
            &[entry(
                "openrouter",
                vec![
                    metric("Credit balance", 25, "$75.00", "$25.00 used"),
                    ReportSection::Spacer,
                    ReportSection::Text {
                        label: "Resets".into(),
                        value: "in 9d".into(),
                    },
                    ReportSection::Block {
                        label: "Usage by period".into(),
                        body: vec!["today $1.00 · week $5.00".into()],
                    },
                ],
            )],
            None,
        );
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let first = &value["entries"][0];
        assert_eq!(first["metrics"].as_array().unwrap().len(), 1);
        assert_eq!(first["sections"][1]["type"], "spacer");
        assert_eq!(first["sections"][2]["type"], "text");
        assert!(first["sections"][2].get("percent").is_none());
        assert_eq!(first["sections"][3]["type"], "block");
        assert_eq!(first["sections"][3]["body"][0], "today $1.00 · week $5.00");
    }

    #[test]
    fn real_panel_projection_keeps_openrouter_blocks() {
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Openrouter(OpenRouterSnapshot {
                label: "OR".into(),
                total_credits: 100.0,
                total_usage: 25.0,
                usage_daily: 1.0,
                usage_weekly: 5.0,
                usage_monthly: 25.0,
                is_free_tier: false,
                limit: None,
                limit_remaining: None,
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
            display: Default::default(),
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Openrouter), &state, Utc::now());
        assert!(projected.sections.iter().any(|section| matches!(
            section,
            ReportSection::Block { label, .. } if label == "Usage by period"
        )));
        assert!(projected.sections.iter().any(|section| matches!(
            section,
            ReportSection::Block { label, .. } if label == "Tier"
        )));
        let text = render_text(&[projected]);
        assert!(text.contains("Usage by period"), "{text}");
        assert!(
            text.contains("today $1.00 · week $5.00 · month $25.00"),
            "{text}"
        );
    }

    #[test]
    fn real_balance_text_is_not_exposed_as_a_percentage_metric() {
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Deepseek(DeepseekSnapshot {
                is_available: true,
                balance: 12.5,
                granted: 2.5,
                topped_up: 10.0,
                currency: "USD".into(),
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
            display: Default::default(),
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Deepseek), &state, Utc::now());
        assert!(projected.sections.iter().any(|section| matches!(
            section,
            ReportSection::Text { label, value } if label == "Balance" && value == "$12.50"
        )));
        assert!(
            !projected
                .sections
                .iter()
                .any(|section| matches!(section, ReportSection::Metric { .. }))
        );
    }

    /// Every metric declares which of its two numbers goes on the bar, in both
    /// the ordered `sections` list and the `metrics` convenience view, so no
    /// frontend has to infer a balance row from its label.
    #[test]
    fn json_metrics_name_their_headline() {
        let deepseek = |display: crate::balance::DisplayPrefs| {
            let state = TabState::Ready(Box::new(ReadyTab {
                snapshot: VendorSnapshot::Deepseek(DeepseekSnapshot {
                    is_available: true,
                    balance: 50.0,
                    granted: 50.0,
                    topped_up: 0.0,
                    currency: "USD".into(),
                }),
                stale: false,
                last_error: None,
                fetched_at: None,
                display,
            }));
            let entry = entry_from_state(&TabId::vendor(VendorId::Deepseek), &state, Utc::now());
            let rendered = render_json_entries(&[entry]);
            serde_json::from_str::<serde_json::Value>(&rendered).unwrap()
        };

        let amount = deepseek(crate::balance::DisplayPrefs::balance(
            Some(200.0),
            crate::balance::Headline::Amount,
        ));
        let metric = &amount["entries"][0]["metrics"][0];
        assert_eq!(metric["headline"], "value");
        assert_eq!(metric["percent"], 75);
        assert_eq!(metric["value"], "$50.00");
        assert!(
            metric["detail"].as_str().unwrap().contains("$200.00"),
            "{metric}"
        );

        let percent = deepseek(crate::balance::DisplayPrefs::balance(
            Some(200.0),
            crate::balance::Headline::Percent,
        ));
        let section = percent["entries"][0]["sections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|section| section["type"] == "metric")
            .expect("a metric section");
        assert_eq!(section["headline"], "percent");
        assert_eq!(section["value"], "75%");
        assert!(
            section["detail"].as_str().unwrap().contains("$50.00"),
            "{section}"
        );

        // A quota vendor is unchanged: still a percent headline.
        let anthropic_api = entry_from_state(
            &TabId::vendor(VendorId::Openrouter),
            &TabState::Ready(Box::new(ReadyTab {
                snapshot: VendorSnapshot::Openrouter(crate::usage::OpenRouterSnapshot {
                    label: "OpenRouter".into(),
                    total_credits: 100.0,
                    total_usage: 40.0,
                    usage_daily: 0.0,
                    usage_weekly: 0.0,
                    usage_monthly: 0.0,
                    is_free_tier: false,
                    limit: None,
                    limit_remaining: None,
                }),
                stale: false,
                last_error: None,
                fetched_at: None,
                display: Default::default(),
            })),
            Utc::now(),
        );
        let value: serde_json::Value =
            serde_json::from_str(&render_json_entries(&[anthropic_api])).unwrap();
        assert_eq!(value["entries"][0]["metrics"][0]["headline"], "percent");
    }

    #[test]
    fn failed_entries_do_not_duplicate_tui_retry_rows() {
        let failed = entry_from_state(
            &TabId::vendor(VendorId::Openai),
            &TabState::error("not \x1b[31msigned in\u{202e}"),
            Utc::now(),
        );
        assert_eq!(failed.error.as_deref(), Some("not [31msigned in"));
        assert!(failed.sections.is_empty());
    }

    #[test]
    fn anthropic_error_keeps_oauth_plan_without_inventing_gauges() {
        let failed = TabState::error_with_plan(
            "HTTP 401: authentication rejected — credentials may be missing, expired, or invalid",
            Some("Claude Max 5x".into()),
        );
        let entry = entry_from_state(&TabId::vendor(VendorId::Anthropic), &failed, Utc::now());
        assert_eq!(entry.plan.as_deref(), Some("Claude Max 5x"));
        assert!(entry.sections.is_empty());
        assert!(entry.error.as_deref().unwrap().contains("401"));
        let rendered = render_json_for_primary(&[entry], None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["entries"][0]["plan"], "Claude Max 5x");
        assert_eq!(value["entries"][0]["status"], "error");
        assert_eq!(value["entries"][0]["sections"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn produced_document_exits_zero_even_when_every_entry_failed() {
        let mut failed = entry("openai", Vec::new());
        failed.error = Some("not signed in".into());
        assert_eq!(report_exit_code(&[failed]), 0);

        let mut failed = entry("openai", Vec::new());
        failed.error = Some("not signed in".into());
        assert_eq!(report_exit_code(&[failed, entry("cursor", Vec::new())]), 0);

        assert_eq!(report_exit_code(&[entry("cursor", Vec::new())]), 0);
        // Empty is not a produced document — collect_entries already fails
        // before this helper when nothing is enabled.
        assert_ne!(report_exit_code(&[]), 0);
    }

    fn custom_spec(id: &str, enabled: bool) -> crate::config::CustomProviderConfig {
        crate::config::CustomProviderConfig {
            id: id.into(),
            name: "My Tool".into(),
            short_name: "myt".into(),
            enabled,
            ..Default::default()
        }
    }

    /// A custom provider can choose a built-in vendor's mark with `brand`; the
    /// report relays that slug without copying config-only data into `TabId`.
    #[test]
    fn a_custom_entry_relays_the_brand_it_borrowed() {
        let spec = crate::config::CustomProviderConfig {
            brand: Some("opencode-go".into()),
            ..custom_spec("oc-second", true)
        };
        let config = Config {
            custom: vec![spec],
            ..Default::default()
        };
        let tab = TabId::custom(&config.custom[0]);
        let entry =
            entry_from_state_with_config(&config, &tab, &TabState::error("HTTP 500"), Utc::now());
        assert_eq!(entry.brand.as_deref(), Some("opencode-go"));
        // The tag is untouched: the mark is artwork, not the bar label.
        assert_eq!(entry.short_name, "myt");

        let value: serde_json::Value =
            serde_json::from_str(&render_json_for_primary(&[entry], None)).unwrap();
        assert_eq!(value["entries"][0]["brand"], "opencode-go");
    }

    /// A `[[custom]]` provider is one more report entry after the built-ins,
    /// addressed as `custom:<id>`; a disabled one is absent.
    #[test]
    fn enabled_custom_providers_are_listed_after_builtins_by_custom_id() {
        use crate::tui::app::tabs_from_config;

        let mut config = Config {
            custom: vec![custom_spec("mytool", true)],
            ..Default::default()
        };
        let tabs = tabs_from_config(&config);
        let ids: Vec<String> = tabs.iter().map(tab_id).collect();
        assert_eq!(ids.last().map(String::as_str), Some("custom:mytool"));
        assert!(
            ids[..ids.len() - 1]
                .iter()
                .all(|id| !id.starts_with("custom:"))
        );
        assert_eq!(tabs.last(), Some(&TabId::custom(&config.custom[0])));

        config.custom[0].enabled = false;
        assert!(
            tabs_from_config(&config)
                .iter()
                .all(|tab| tab_id(tab) != "custom:mytool")
        );
    }

    #[test]
    fn custom_entries_carry_their_configured_names_and_projected_windows() {
        use crate::custom::types::{CustomMetric, CustomSnapshot, CustomText};

        let now = Utc::now();
        let spec = custom_spec("mytool", true);
        let tab = TabId::custom(&spec);
        assert_eq!(tab_id(&tab), "custom:mytool");
        assert_eq!(tab_name(&tab), "My Tool");
        assert_eq!(tab_display_name(&tab), "My Tool");

        let session_reset = now + chrono::Duration::hours(2);
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Custom(CustomSnapshot {
                plan: Some("Team".into()),
                metrics: vec![
                    CustomMetric {
                        label: "Session".into(),
                        pct: 40,
                        footnote: "40 of 100".into(),
                        resets_at: Some(session_reset),
                        window_secs: Some(18_000),
                    },
                    CustomMetric {
                        label: "Monthly".into(),
                        pct: 10,
                        footnote: String::new(),
                        resets_at: None,
                        window_secs: None,
                    },
                ],
                texts: vec![CustomText {
                    label: "Region".into(),
                    value: "eu".into(),
                }],
            }),
            stale: false,
            last_error: None,
            fetched_at: Some(now),
            display: Default::default(),
        }));
        let projected = entry_from_state(&tab, &state, now);
        assert_eq!(projected.id, "custom:mytool");
        assert_eq!(projected.display_name, "My Tool");
        assert_eq!(projected.short_name, "myt");
        assert_eq!(projected.plan.as_deref(), Some("Team"));

        let rendered = render_json_for_primary(&[projected], None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let first = &value["entries"][0];
        assert_eq!(first["short_name"], "myt");
        assert_eq!(first["icon"], "myt");
        // No `brand`, so the optional field is absent and the frontend keeps
        // drawing the short_name tag.
        assert!(first.get("brand").is_none());
        assert_eq!(first["metrics"][0]["label"], "Session");
        assert_eq!(first["metrics"][0]["percent"], 40);
        assert_eq!(first["metrics"][0]["window_secs"], 18_000);
        assert_eq!(
            first["metrics"][0]["reset_at"],
            session_reset.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
        );
        assert_eq!(first["metrics"][1]["label"], "Monthly");
        assert!(first["metrics"][1].get("window_secs").is_none());
        assert!(first["sections"].as_array().unwrap().iter().any(|section| {
            section["type"] == "text" && section["label"] == "Region" && section["value"] == "eu"
        }));

        // A failed custom entry still carries its code and names.
        let failed = entry_from_state(&tab, &TabState::error("HTTP 500"), now);
        assert_eq!(failed.short_name, "myt");
        assert_eq!(failed.display_name, "My Tool");
        assert!(failed.sections.is_empty());
    }
}
