//! Native ratatui panels.
//!
//! Each vendor projects its snapshot into a sequence of [`Section`]s — either
//! a metric (gauge + footnote) or a free-form text block. The renderer lays
//! them out vertically with consistent spacing so every panel has the same
//! visual rhythm regardless of vendor.
//!
//! Progress bars use Bubble Tea-style block glyphs that scale to the available
//! width, so on a wide monitor you get long, readable bars instead of the
//! 20-char Pango ones the Waybar tooltip is stuck with.

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui_bubbletea_components::{Progress, Spinner, SpinnerFrames};
use ratatui_bubbletea_theme::BubbleTheme;

use crate::countdown;
use crate::format::{local_time_hms, money, reset_credit_lines, usd};
use crate::pacing::{self, PaceSeverity};
use crate::pango::severity_for;
use crate::theme::Theme;
use crate::tui::app::TabState;
use crate::tui::style::{bubble_theme, color, progress_theme, severity_color};
use crate::usage::VendorSnapshot;

/// One row of the panel body. Vendors emit a `Vec<Section>`; the renderer
/// turns them into ratatui widgets.
pub enum Section {
    /// Title row at the top. `left` is the plan/vendor label (accent-colored,
    /// bold); `right` is an optional right-aligned annotation, used for the
    /// "Updated HH:MM:SS" timestamp so it shares the title row instead of
    /// taking a separate body row + duplicating the global footer's clock.
    Title { left: String, right: Option<String> },
    /// A metric: label + gauge + value annotation + dim footnote.
    Metric {
        label: String,
        pct: u16,
        severity: PaceSeverity,
        value_label: String,
        footnote: String,
    },
    /// Free-form key/value text line.
    Text { label: String, value: String },
    /// A label followed by a multi-line dim block (no gauge).
    Block { label: String, body: Vec<String> },
    /// Visual spacer (one blank row).
    Spacer,
}

/// Internal metadata carried alongside a public [`Section`]. Keeping this
/// wrapper private to the crate lets machine-readable frontends receive
/// absolute reset timestamps without adding a source-breaking field to the
/// public `Section::Metric` variant.
pub(crate) struct SectionProjection {
    pub section: Section,
    pub reset_at: Option<DateTime<Utc>>,
}

struct SectionBuilder(Vec<SectionProjection>);

impl SectionBuilder {
    fn new(sections: Vec<Section>) -> Self {
        Self(
            sections
                .into_iter()
                .map(|section| {
                    assert!(
                        !matches!(section, Section::Metric { .. }),
                        "metric sections must declare reset metadata with push_metric"
                    );
                    SectionProjection {
                        section,
                        reset_at: None,
                    }
                })
                .collect(),
        )
    }

    fn push(&mut self, section: Section) {
        assert!(
            !matches!(section, Section::Metric { .. }),
            "metric sections must declare reset metadata with push_metric"
        );
        self.0.push(SectionProjection {
            section,
            reset_at: None,
        });
    }

    fn push_metric(&mut self, section: Section, reset_at: Option<DateTime<Utc>>) {
        assert!(matches!(section, Section::Metric { .. }));
        self.0.push(SectionProjection { section, reset_at });
    }
}

/// Compact one-line projection of a vendor snapshot for the Overview: a short
/// plan/tier sub-label (may be empty) plus a few key metric cells — a percent
/// or a balance — each carrying a severity for coloring. Same numbers as
/// [`sections_for`], flattened for a dense multi-vendor list. The vendor's name
/// is supplied by the caller, so it is not repeated here.
pub fn compact_cells(snapshot: &VendorSnapshot) -> (String, Vec<(String, PaceSeverity)>) {
    let pct = |label: &str, p: i32| (format!("{label} {p}%"), severity_for(p));
    // Named `*_cell` so neither shadows `format::{usd, money}`, which they
    // wrap — the cell is the string plus the severity the Overview colours it with.
    let usd_cell = |v: f64| (usd(v), PaceSeverity::Low);
    let money_cell = |v: f64, c: &str| (money(v, c), PaceSeverity::Low);
    let (plan, mut cells) = match snapshot {
        VendorSnapshot::Anthropic(s) => {
            let mut cells = vec![
                pct("S", s.session.utilization_pct),
                pct("W", s.weekly.utilization_pct),
            ];
            if let Some(sonnet) = &s.sonnet {
                cells.push(pct("Son", sonnet.utilization_pct));
            }
            (s.plan.clone(), cells)
        }
        VendorSnapshot::AnthropicApi(s) => {
            let cell = match s.pct() {
                Some(p) => pct("spend", p),
                None => (format!("{}/mo", usd(s.spent)), PaceSeverity::Low),
            };
            (String::new(), vec![cell])
        }
        VendorSnapshot::Openai(s) => {
            let mut cells = Vec::new();
            if let Some(w) = &s.session {
                cells.push(pct("5h", w.utilization_pct));
            }
            if let Some(w) = &s.weekly {
                cells.push(pct("7d", w.utilization_pct));
            }
            if cells.is_empty() {
                cells.push(("—".into(), PaceSeverity::Low));
            }
            (s.plan.clone(), cells)
        }
        VendorSnapshot::Copilot(s) => (
            s.plan.clone(),
            s.quotas()
                .map(|(label, quota)| pct(label, quota.used_pct()))
                .collect(),
        ),
        VendorSnapshot::Zai(s) => {
            let mut cells = Vec::new();
            if let Some(w) = &s.session {
                cells.push(pct("S", w.utilization_pct));
            }
            if let Some(w) = &s.weekly {
                cells.push(pct("W", w.utilization_pct));
            }
            if cells.is_empty() {
                cells.push(("—".into(), PaceSeverity::Low));
            }
            (s.plan.clone(), cells)
        }
        VendorSnapshot::Openrouter(s) => (String::new(), vec![usd_cell(s.balance())]),
        VendorSnapshot::Deepseek(s) => (String::new(), vec![money_cell(s.balance, &s.currency)]),
        VendorSnapshot::Kimi(s) => (
            s.plan.clone().unwrap_or_default(),
            vec![pct("5h", s.window_pct()), pct("wk", s.weekly_pct())],
        ),
        VendorSnapshot::Kilo(s) => (String::new(), vec![usd_cell(s.balance)]),
        VendorSnapshot::Novita(s) => (String::new(), vec![usd_cell(s.available)]),
        VendorSnapshot::Moonshot(s) => (String::new(), vec![money_cell(s.available, &s.currency)]),
        VendorSnapshot::Grok(s) => (String::new(), vec![usd_cell(s.balance)]),
        VendorSnapshot::SuperGrok(s) => (s.plan.clone(), vec![pct(s.period.short(), s.weekly_pct)]),
        VendorSnapshot::Antigravity(s) => (
            s.plan.clone(),
            [
                s.session.as_ref().map(|w| pct("S", w.utilization_pct)),
                s.weekly.as_ref().map(|w| pct("W", w.utilization_pct)),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ),
        VendorSnapshot::Cursor(s) => (
            s.plan.clone(),
            vec![pct("auto", s.auto_pct), pct("premium", s.api_pct)],
        ),
        VendorSnapshot::Minimax(s) => (
            s.plan.clone(),
            vec![
                pct("S", s.session.utilization_pct),
                pct("W", s.weekly.utilization_pct),
            ],
        ),
        VendorSnapshot::Kiro(s) => (s.plan.clone(), vec![pct("credits", s.pct())]),
        VendorSnapshot::NousResearch(s) => {
            let cell = s
                .usage_percent()
                .map(|value| pct("usage", value.round().clamp(0.0, 100.0) as i32))
                .unwrap_or_else(|| ("—".into(), PaceSeverity::Low));
            (s.plan.clone().unwrap_or_default(), vec![cell])
        }
        VendorSnapshot::CommandCode(s) => {
            let cells = [
                ("session", s.five_hour.as_ref()),
                ("weekly", s.weekly.as_ref()),
            ]
            .into_iter()
            .filter_map(|(label, window)| window.map(|window| pct(label, window.pct())))
            .collect();
            (s.plan.clone().unwrap_or_default(), cells)
        }
        VendorSnapshot::OpenCodeGo(s) => {
            let cells = [
                ("rolling", s.rolling.as_ref()),
                ("weekly", s.weekly.as_ref()),
                ("monthly", s.monthly.as_ref()),
            ]
            .into_iter()
            .filter_map(|(label, window)| {
                window.map(|window| pct(label, window.percent.round().clamp(0.0, 100.0) as i32))
            })
            .collect();
            ("OpenCode Go".into(), cells)
        }
    };

    for (text, _) in &mut cells {
        *text = crate::display::sanitize_untrusted_field(text);
    }
    (crate::display::sanitize_untrusted_field(&plan), cells)
}

/// The single most-relevant percentage for a vendor in the Overview — what its
/// per-row mini bar shows. Mirrors the macOS menu bar's headline: Cursor is the
/// combined included-total, quota vendors the most-exhausted window; balance
/// vendors have no meaningful percentage (`None` → no bar).
pub fn headline_pct(snapshot: &VendorSnapshot) -> Option<i32> {
    match snapshot {
        VendorSnapshot::Anthropic(s) => [
            Some(s.session.utilization_pct),
            Some(s.weekly.utilization_pct),
            s.sonnet.as_ref().map(|w| w.utilization_pct),
        ]
        .into_iter()
        .flatten()
        .max(),
        VendorSnapshot::AnthropicApi(s) => s.pct(),
        VendorSnapshot::Openai(s) => [
            s.session.as_ref().map(|w| w.utilization_pct),
            s.weekly.as_ref().map(|w| w.utilization_pct),
        ]
        .into_iter()
        .flatten()
        .max(),
        VendorSnapshot::Copilot(s) => s.quotas().map(|(_, quota)| quota.used_pct()).max(),
        VendorSnapshot::Zai(s) => [
            s.session.as_ref().map(|w| w.utilization_pct),
            s.weekly.as_ref().map(|w| w.utilization_pct),
        ]
        .into_iter()
        .flatten()
        .max(),
        VendorSnapshot::Kimi(s) => Some(s.weekly_pct().max(s.window_pct())),
        VendorSnapshot::Antigravity(s) => [
            s.session.as_ref().map(|w| w.utilization_pct),
            s.weekly.as_ref().map(|w| w.utilization_pct),
            s.third_party_session.as_ref().map(|w| w.utilization_pct),
            s.third_party_weekly.as_ref().map(|w| w.utilization_pct),
        ]
        .into_iter()
        .flatten()
        .max(),
        VendorSnapshot::Cursor(s) => (!s.unlimited).then_some(s.total_pct),
        VendorSnapshot::Minimax(s) => Some(s.session.utilization_pct.max(s.weekly.utilization_pct)),
        VendorSnapshot::Kiro(s) => Some(s.pct()),
        VendorSnapshot::NousResearch(s) => s
            .usage_percent()
            .map(|value| value.round().clamp(0.0, 100.0) as i32),
        VendorSnapshot::CommandCode(s) => {
            let worst = s.worst_pct();
            (s.five_hour.is_some() || s.weekly.is_some()).then_some(worst)
        }
        VendorSnapshot::OpenCodeGo(s) => [
            s.rolling
                .as_ref()
                .map(|window| window.percent.round() as i32),
            s.weekly
                .as_ref()
                .map(|window| window.percent.round() as i32),
            s.monthly
                .as_ref()
                .map(|window| window.percent.round() as i32),
        ]
        .into_iter()
        .flatten()
        .max(),
        VendorSnapshot::SuperGrok(s) => Some(s.weekly_pct),
        VendorSnapshot::Openrouter(_)
        | VendorSnapshot::Deepseek(_)
        | VendorSnapshot::Kilo(_)
        | VendorSnapshot::Novita(_)
        | VendorSnapshot::Moonshot(_)
        | VendorSnapshot::Grok(_) => None,
    }
}

/// Build the section list for the currently-active vendor's snapshot.
pub fn sections_for(tab: &TabState, now: DateTime<Utc>, pace_tolerance: u32) -> Vec<Section> {
    sections_with_metadata_for(tab, now, pace_tolerance)
        .into_iter()
        .map(|projected| projected.section)
        .collect()
}

/// Rich projection used by machine-readable frontends. The TUI continues to
/// expose the source-compatible [`sections_for`] result above.
pub(crate) fn sections_with_metadata_for(
    tab: &TabState,
    now: DateTime<Utc>,
    pace_tolerance: u32,
) -> Vec<SectionProjection> {
    let mut sections = match tab {
        TabState::Loading => SectionBuilder::new(vec![
            Section::Spacer,
            Section::Text {
                label: "".into(),
                value: "  Loading…".into(),
            },
        ]),
        TabState::Error(e) => SectionBuilder::new(vec![
            Section::Spacer,
            Section::Text {
                label: "Error".into(),
                value: e.clone(),
            },
            Section::Spacer,
            Section::Text {
                label: "".into(),
                value: "Press `r` to retry, `q` to quit.".into(),
            },
        ]),
        TabState::Ready(r) => {
            let snapshot = &r.snapshot;
            let last_error = &r.last_error;
            let mut sections = match snapshot {
                VendorSnapshot::Anthropic(s) => anthropic_sections(s, now, pace_tolerance),
                VendorSnapshot::AnthropicApi(s) => anthropic_api_sections(s),
                VendorSnapshot::Openai(s) => openai_sections(s, now, pace_tolerance),
                VendorSnapshot::Copilot(s) => copilot_sections(s, now),
                VendorSnapshot::Zai(s) => zai_sections(s, now, pace_tolerance),
                VendorSnapshot::Openrouter(s) => openrouter_sections(s),
                VendorSnapshot::Deepseek(s) => deepseek_sections(s),
                VendorSnapshot::Kimi(s) => kimi_sections(s, now, pace_tolerance),
                VendorSnapshot::Kilo(s) => kilo_sections(s),
                VendorSnapshot::Novita(s) => novita_sections(s),
                VendorSnapshot::Moonshot(s) => moonshot_sections(s),
                VendorSnapshot::Grok(s) => grok_sections(s),
                VendorSnapshot::SuperGrok(s) => supergrok_sections(s, now),
                VendorSnapshot::Antigravity(s) => antigravity_sections(s, now),
                VendorSnapshot::Cursor(s) => cursor_sections(s, now),
                VendorSnapshot::Minimax(s) => minimax_sections(s, now, pace_tolerance),
                VendorSnapshot::Kiro(s) => kiro_sections(s, now),
                VendorSnapshot::NousResearch(s) => nous_sections(s, now),
                VendorSnapshot::OpenCodeGo(s) => opencode_go_sections(s, now),
                VendorSnapshot::CommandCode(s) => commandcode_sections(s, now),
            };
            // Inject the (already-absolute) fetched-at instant into the title
            // row, right-aligned. Pre-snapshotted in app::refresh_one so it
            // doesn't drift between redraws.
            let updated = match r.fetched_at {
                Some(at) => format!("Updated {}", local_time_hms(at)),
                None => "Updated —".to_string(),
            };
            if let Some(SectionProjection {
                section: Section::Title { right, .. },
                ..
            }) = sections.0.first_mut()
            {
                *right = Some(updated);
            }
            // Error footer (when present) still lives in the body.
            if let Some((label, msg)) = warning_label(snapshot, last_error) {
                sections.push(Section::Spacer);
                sections.push(Section::Text { label, value: msg });
            }
            sections
        }
    };
    for projected in &mut sections.0 {
        sanitize_section(&mut projected.section);
    }
    sections.0
}

/// Sanitize at the final projection boundary so every vendor field, cached
/// diagnostic, and fetch error is inert before ratatui writes it to a terminal.
fn sanitize_section(section: &mut Section) {
    let clean = |value: &mut String| {
        *value = crate::display::sanitize_untrusted_field(value);
    };
    match section {
        Section::Title { left, right } => {
            clean(left);
            if let Some(right) = right {
                clean(right);
            }
        }
        Section::Metric {
            label,
            value_label,
            footnote,
            ..
        } => {
            clean(label);
            clean(value_label);
            clean(footnote);
        }
        Section::Text { label, value } => {
            clean(label);
            clean(value);
        }
        Section::Block { label, body } => {
            clean(label);
            for line in body {
                clean(line);
            }
        }
        Section::Spacer => {}
    }
}

/// Translate cache diagnostics at the presentation boundary. Cache files keep
/// their established `(u16, String)` form: only non-zero codes are HTTP, while
/// Kimi's stable schema marker identifies its code-zero schema warning.
fn warning_label(
    snapshot: &VendorSnapshot,
    last_error: &Option<(u16, String)>,
) -> Option<(String, String)> {
    let (code, message) = last_error.as_ref()?;
    if *code != 0 {
        return Some((format!("HTTP {code}"), message.clone()));
    }
    if message.is_empty() {
        return None;
    }
    let label = if matches!(snapshot, VendorSnapshot::Kimi(_))
        && matches!(
            crate::kimi::vendor::warning_kind(*code, message),
            crate::kimi::vendor::WarningKind::SchemaDrift
        ) {
        "Kimi API schema drift"
    } else {
        "Warning"
    };
    // The stable marker is already the schema-warning label. Keep the label
    // visible but do not repeat that sentinel as a redundant body value.
    let value = if label == message {
        String::new()
    } else {
        message.clone()
    };
    Some((label.into(), value))
}

fn anthropic_api_sections(s: &crate::usage::AnthropicApiSnapshot) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: "Anthropic API".into(),
        right: None,
    }]);
    match (s.limit.filter(|l| *l > 0.0), s.pct()) {
        (Some(limit), Some(pct)) => {
            let p = pct.clamp(0, 100) as u16;
            v.push_metric(
                Section::Metric {
                    label: "Spend (mo)".into(),
                    pct: p,
                    severity: severity_for(pct),
                    value_label: format!("{} of ${:.0}", usd(s.spent), limit),
                    footnote: format!("{pct}% of monthly limit"),
                },
                None,
            );
        }
        _ => {
            v.push(Section::Text {
                label: "Spend (mo)".into(),
                value: usd(s.spent),
            });
        }
    }
    v.push(Section::Spacer);
    v.push(Section::Text {
        label: "".into(),
        value: "Month-to-date cost via the Admin usage API.".into(),
    });
    v.push(Section::Text {
        label: "".into(),
        value: "Prepaid credit balance is Console-only (no API).".into(),
    });
    v.push(Section::Text {
        label: "".into(),
        value: "Excludes Priority Tier cost (not reported by this API).".into(),
    });
    v
}

fn anthropic_sections(
    s: &crate::usage::AnthropicSnapshot,
    now: DateTime<Utc>,
    tol: u32,
) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: format!("Claude {}", s.plan),
        right: None,
    }]);

    push_window(&mut v, "Session (5h)", &s.session, now, tol, true);
    push_window(&mut v, "Weekly (7d)", &s.weekly, now, tol, true);
    if let Some(w) = &s.sonnet {
        push_window(&mut v, "Sonnet only", w, now, tol, false);
    }
    for sw in &s.scoped {
        push_window(
            &mut v,
            &format!("{} (7d)", sw.label),
            &sw.window,
            now,
            tol,
            false,
        );
    }
    if let Some(e) = &s.extra {
        v.push(Section::Spacer);
        let pct = e.percent().clamp(0, 100) as u16;
        // An uncapped plan (`monthly_limit: null`) has spend but no
        // denominator: show the amount alone rather than "of $0.00" or a
        // percentage nobody can vouch for (#30).
        let (value_label, footnote) = match e.fmt_limit() {
            Some(l) => (
                format!("{} of {}", e.fmt_spent(), l),
                format!("{pct}% of monthly limit consumed"),
            ),
            None => (e.fmt_spent(), "no monthly limit reported".to_string()),
        };
        v.push_metric(
            Section::Metric {
                label: "Extra usage".into(),
                pct,
                severity: severity_for(pct as i32),
                value_label,
                footnote,
            },
            None,
        );
    }
    v
}

fn openai_sections(
    s: &crate::usage::OpenAiSnapshot,
    now: DateTime<Utc>,
    tol: u32,
) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: s.plan.clone(),
        right: None,
    }]);
    if let Some(session) = &s.session {
        push_window(&mut v, "Codex 5h", session, now, tol, true);
    }
    if let Some(weekly) = &s.weekly {
        push_window(&mut v, "Codex weekly", weekly, now, tol, true);
    }
    if s.session.is_none() && s.weekly.is_none() {
        v.push(Section::Spacer);
        v.push(Section::Text {
            label: "".into(),
            value: "  no usage windows reported".into(),
        });
    }
    if let Some(cr) = &s.code_review {
        push_window(&mut v, "Code review", cr, now, tol, false);
    }
    // A named limit can be the binding one while the headline window reads
    // low, so it gets a real row rather than a footnote.
    for limit in &s.additional_limits {
        if let Some(w) = &limit.session {
            push_window(&mut v, &format!("{} (5h)", limit.name), w, now, tol, false);
        }
        if let Some(w) = &limit.weekly {
            push_window(&mut v, &format!("{} (7d)", limit.name), w, now, tol, false);
        }
    }
    // No percentage reflects a model the account cannot dispatch to, so say it
    // outright rather than leaving the user to infer it from healthy bars.
    if !s.unavailable_models.is_empty() {
        v.push(Section::Spacer);
        v.push(Section::Block {
            label: "Unavailable".into(),
            body: s
                .unavailable_models
                .iter()
                .map(|m| match m.available_at {
                    Some(at) => format!("{} — back {}", m.model, countdown::format(Some(at), now)),
                    None => format!("{} — at capacity", m.model),
                })
                .collect(),
        });
    }
    if let Some(c) = &s.credits {
        v.push(Section::Spacer);
        let balance = if c.unlimited {
            "unlimited".into()
        } else {
            c.balance.clone()
        };
        let mut body = vec![format!("balance: {}", balance)];
        if let Some((lo, hi)) = c.approx_local_messages {
            body.push(format!("≈ {lo}-{hi} local messages"));
        }
        if let Some((lo, hi)) = c.approx_cloud_messages {
            body.push(format!("≈ {lo}-{hi} cloud messages"));
        }
        v.push(Section::Block {
            label: "Credits".into(),
            body,
        });
    }
    push_reset_credits(&mut v, &s.reset_credits, now);
    v
}

fn copilot_sections(s: &crate::copilot::types::Snapshot, now: DateTime<Utc>) -> SectionBuilder {
    let mut sections = SectionBuilder::new(vec![Section::Title {
        left: format!("GitHub Copilot {}", s.plan),
        right: None,
    }]);
    for (label, quota) in s.quotas() {
        let pct = quota.used_pct();
        let detail = if quota.unlimited {
            "Unlimited".to_string()
        } else {
            quota
                .used_and_entitlement()
                .map(|(used, entitlement)| format!("{used} of {entitlement} used"))
                .unwrap_or_else(|| format!("{}% remaining", quota.percent_remaining))
        };
        sections.push(Section::Spacer);
        sections.push_metric(
            Section::Metric {
                label: label.to_string(),
                pct: pct.clamp(0, 100) as u16,
                severity: severity_for(pct),
                value_label: if quota.unlimited {
                    "Unlimited".to_string()
                } else {
                    format!("{pct}%")
                },
                footnote: detail,
            },
            s.reset_at,
        );
    }
    sections.push(Section::Spacer);
    sections.push(Section::Text {
        label: "Resets".into(),
        value: countdown::format(s.reset_at, now),
    });
    sections
}

fn zai_sections(s: &crate::usage::ZaiSnapshot, now: DateTime<Utc>, tol: u32) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: s.plan.clone(),
        right: None,
    }]);
    if let Some(w) = &s.session {
        push_window(&mut v, "Session (5h)", w, now, tol, true);
    }
    if let Some(w) = &s.weekly {
        push_window(&mut v, "Weekly", w, now, tol, true);
    }
    if let Some(w) = &s.mcp {
        push_window(&mut v, "MCP tools (monthly)", w, now, tol, true);
    }
    if s.session.is_none() && s.weekly.is_none() && s.mcp.is_none() {
        v.push(Section::Spacer);
        v.push(Section::Text {
            label: "".into(),
            value: "  no usage windows reported".into(),
        });
    }
    v
}

fn openrouter_sections(s: &crate::usage::OpenRouterSnapshot) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: s.label.clone(),
        right: None,
    }]);
    let pct = s.consumed_pct().clamp(0, 100) as u16;
    v.push(Section::Spacer);
    v.push_metric(
        Section::Metric {
            label: "Credit balance".into(),
            pct,
            // One severity policy for every frontend: this value is what the
            // Omarchy, GNOME and KDE panels colour their row with, so it has to
            // agree with the Waybar tooltip about what "in debt" looks like.
            severity: crate::openrouter::vendor::severity(s),
            value_label: usd(s.balance()),
            footnote: format!(
                "{} of {} used ({pct}%)",
                usd(s.total_usage),
                usd(s.total_credits)
            ),
        },
        None,
    );
    v.push(Section::Spacer);
    v.push(Section::Block {
        label: "Usage by period".into(),
        body: vec![format!(
            "today ${:.2} · week ${:.2} · month ${:.2}",
            s.usage_daily, s.usage_weekly, s.usage_monthly
        )],
    });
    if let (Some(limit), Some(rem)) = (s.limit, s.limit_remaining) {
        v.push(Section::Spacer);
        v.push(Section::Block {
            label: "Per-key limit".into(),
            body: vec![format!("{} of {} remaining", usd(rem), usd(limit))],
        });
    }
    v.push(Section::Spacer);
    v.push(Section::Block {
        label: "Tier".into(),
        body: vec![if s.is_free_tier {
            "free tier".into()
        } else {
            "paid tier".into()
        }],
    });
    v
}

/// Antigravity holds two independent pools (Gemini, Claude & GPT OSS), each
/// with a 5-hour and a weekly window. Grouped by window type so the two pools
/// sit side by side, matching the GNOME dropdown.
fn antigravity_sections(
    s: &crate::usage::AntigravitySnapshot,
    now: DateTime<Utc>,
) -> SectionBuilder {
    use crate::antigravity::vendor::{GROUP_PRIMARY, GROUP_THIRD_PARTY};

    let mut v = SectionBuilder::new(vec![Section::Title {
        left: s.plan.clone(),
        right: None,
    }]);
    for (heading, primary, third_party) in [
        (
            "Session",
            s.session.as_ref(),
            s.third_party_session.as_ref(),
        ),
        ("Weekly", s.weekly.as_ref(), s.third_party_weekly.as_ref()),
    ] {
        // A cadence no bucket reported gets no heading either — an empty
        // "Session" with nothing under it reads as a failed fetch.
        if primary.is_none() && third_party.is_none() {
            continue;
        }
        v.push(Section::Spacer);
        v.push(Section::Text {
            label: heading.into(),
            value: String::new(),
        });
        if let Some(w) = primary {
            push_window(&mut v, GROUP_PRIMARY, w, now, 5, false);
        }
        if let Some(w) = third_party {
            push_window(&mut v, GROUP_THIRD_PARTY, w, now, 5, false);
        }
    }
    v
}

fn cursor_sections(s: &crate::usage::CursorSnapshot, now: DateTime<Utc>) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: format!("Cursor {}", s.plan),
        right: None,
    }]);
    if s.unlimited {
        v.push(Section::Spacer);
        v.push(Section::Text {
            label: "Plan".into(),
            value: "Unlimited — pools don't cap".into(),
        });
    } else {
        // Two included-usage pools, mirroring the dashboard's two bars.
        v.push(Section::Spacer);
        v.push_metric(
            Section::Metric {
                label: "Cursor Models".into(),
                pct: s.auto_pct.clamp(0, 100) as u16,
                severity: severity_for(s.auto_pct),
                value_label: format!("{}%", s.auto_pct),
                footnote: "Auto + Composer".into(),
            },
            s.reset_at,
        );
        v.push(Section::Spacer);
        v.push_metric(
            Section::Metric {
                label: "Other Models".into(),
                pct: s.api_pct.clamp(0, 100) as u16,
                severity: severity_for(s.api_pct),
                value_label: format!("{}%", s.api_pct),
                footnote: format!(
                    "Named / API models · on-demand {}",
                    if s.on_demand_enabled { "on" } else { "off" }
                ),
            },
            s.reset_at,
        );
    }
    v.push(Section::Spacer);
    v.push(Section::Text {
        label: "Resets".into(),
        value: countdown::format(s.reset_at, now),
    });
    v
}

fn nous_sections(s: &crate::nous::types::AccountSnapshot, now: DateTime<Utc>) -> SectionBuilder {
    let mut sections = SectionBuilder::new(vec![Section::Title {
        left: "Nous Research".into(),
        right: None,
    }]);
    if let Some(value) = s.usage_percent() {
        let pct = value.round().clamp(0.0, 100.0) as i32;
        sections.push_metric(
            Section::Metric {
                label: "Usage".into(),
                pct: pct as u16,
                severity: severity_for(pct),
                value_label: format!("{pct}%"),
                footnote: "current period".into(),
            },
            s.current_period_end,
        );
    }
    sections.push(Section::Spacer);
    if let Some(remaining) = s.credits_remaining {
        sections.push(Section::Text {
            label: "Subscription credits".into(),
            value: format!("{remaining:.2} remaining"),
        });
    }
    if let Some(purchased) = s.purchased_credits_remaining {
        sections.push(Section::Text {
            label: "Top-up credits".into(),
            value: format!("{purchased:.2} remaining"),
        });
    }
    if let Some(total_usable) = s.total_usable_credits {
        sections.push(Section::Text {
            label: "Total usable credits".into(),
            value: format!("{total_usable:.2}"),
        });
    }
    if let Some(period_end) = s.current_period_end {
        sections.push(Section::Text {
            label: "Renews".into(),
            value: countdown::format(Some(period_end), now),
        });
    }
    sections
}

fn commandcode_sections(
    s: &crate::commandcode::types::Snapshot,
    now: DateTime<Utc>,
) -> SectionBuilder {
    let title = match s.plan.as_deref() {
        Some(plan) if !plan.is_empty() => format!("Command Code {plan}"),
        _ => "Command Code".to_string(),
    };
    let mut sections = SectionBuilder::new(vec![Section::Title {
        left: title,
        right: None,
    }]);
    for (label, window) in [
        ("Session (5h)", s.five_hour.as_ref()),
        ("Weekly", s.weekly.as_ref()),
    ] {
        if let Some(window) = window {
            let pct = window.pct();
            sections.push_metric(
                Section::Metric {
                    label: label.into(),
                    pct: pct.clamp(0, 100) as u16,
                    severity: severity_for(pct),
                    value_label: format!("{pct}%"),
                    footnote: format!("{} of {}", usd(window.used), usd(window.cap)),
                },
                window.resets_at,
            );
            sections.push(Section::Text {
                label: "Resets".into(),
                value: countdown::format(window.resets_at, now),
            });
        }
    }
    if let Some(credits) = s.credits.as_ref() {
        sections.push(Section::Spacer);
        let footnote = match (s.credits_spent(), s.credit_pool) {
            (Some(spent), Some(pool)) => format!("{} of {} spent", usd(spent), usd(pool)),
            _ => String::new(),
        };
        sections.push(Section::Text {
            label: "Credits".into(),
            value: if footnote.is_empty() {
                usd(credits.remaining())
            } else {
                format!("{} · {footnote}", usd(credits.remaining()))
            },
        });
    }
    sections
}

fn opencode_go_sections(
    s: &crate::opencode_go::types::Usage,
    now: DateTime<Utc>,
) -> SectionBuilder {
    let mut sections = SectionBuilder::new(vec![Section::Title {
        left: "OpenCode Go".into(),
        right: None,
    }]);
    for (label, window) in [
        ("Rolling", s.rolling.as_ref()),
        ("Weekly", s.weekly.as_ref()),
        ("Monthly", s.monthly.as_ref()),
    ] {
        if let Some(window) = window {
            let pct = window.percent.round().clamp(0.0, 100.0) as i32;
            sections.push_metric(
                Section::Metric {
                    label: label.into(),
                    pct: pct as u16,
                    severity: severity_for(pct),
                    value_label: format!("{pct}%"),
                    footnote: String::new(),
                },
                Some(window.resets_at),
            );
            sections.push(Section::Text {
                label: "Resets".into(),
                value: countdown::format(Some(window.resets_at), now),
            });
        }
    }
    sections
}

/// Kiro has a single credit pool, so the panel is a single metric bar plus
/// the reset row — the same shape as `anthropic_api_sections` but with a
/// real percentage (Kiro always reports both used and limit) instead of an
/// optional configured one.
fn kiro_sections(s: &crate::usage::KiroSnapshot, now: DateTime<Utc>) -> SectionBuilder {
    let pct = s.pct();
    let mut v = SectionBuilder::new(vec![
        Section::Title {
            left: format!("Kiro {}", s.plan),
            right: None,
        },
        Section::Spacer,
    ]);
    v.push_metric(
        Section::Metric {
            label: "Credits".into(),
            pct: pct.clamp(0, 100) as u16,
            severity: severity_for(pct),
            value_label: format!("{pct}%"),
            footnote: format!("{:.2} of {:.0}", s.used, s.limit),
        },
        s.reset_at,
    );
    v.push(Section::Spacer);
    v.push(Section::Text {
        label: "Resets".into(),
        value: countdown::format(s.reset_at, now),
    });
    v
}

/// MiniMax groups quota by model bucket, so the panel is laid out by window
/// (Session, Weekly) with one row per pool — the same shape as Antigravity's
/// two-group panel. Pacing is shown: both windows report a real duration, so
/// the marker is meaningful.
fn minimax_sections(
    s: &crate::usage::MinimaxSnapshot,
    now: DateTime<Utc>,
    tol: u32,
) -> SectionBuilder {
    use crate::minimax::vendor::{POOL_GENERAL, POOL_VIDEO};

    let mut v = SectionBuilder::new(vec![Section::Title {
        left: s.plan.clone(),
        right: None,
    }]);
    for (heading, general, video) in [
        ("Session", &s.session, s.video_session.as_ref()),
        ("Weekly", &s.weekly, s.video_weekly.as_ref()),
    ] {
        v.push(Section::Spacer);
        v.push(Section::Text {
            label: heading.into(),
            value: String::new(),
        });
        push_window(&mut v, POOL_GENERAL, general, now, tol, true);
        if let Some(w) = video {
            push_window(&mut v, POOL_VIDEO, w, now, tol, true);
        }
    }
    v
}

fn kilo_sections(s: &crate::usage::KiloSnapshot) -> SectionBuilder {
    SectionBuilder::new(vec![
        Section::Title {
            left: s.label.clone(),
            right: None,
        },
        Section::Spacer,
        Section::Text {
            label: "Balance".into(),
            value: usd(s.balance),
        },
    ])
}

fn novita_sections(s: &crate::usage::NovitaSnapshot) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![
        Section::Title {
            left: "Novita".into(),
            right: None,
        },
        Section::Spacer,
        Section::Text {
            label: "Balance".into(),
            value: usd(s.available),
        },
        Section::Block {
            label: "Breakdown".into(),
            body: vec![format!(
                "top-up ${:.2} · credit limit ${:.2}",
                s.cash, s.credit_limit
            )],
        },
    ]);
    if s.outstanding > 0.0 {
        v.push(Section::Spacer);
        v.push(Section::Block {
            label: "Owed".into(),
            body: vec![usd(s.outstanding)],
        });
    }
    v
}

fn moonshot_sections(s: &crate::usage::MoonshotSnapshot) -> SectionBuilder {
    let cur = &s.currency;
    let fmt = |v: f64| money(v, cur);
    SectionBuilder::new(vec![
        Section::Title {
            left: "Kimi (Moonshot)".into(),
            right: None,
        },
        Section::Spacer,
        Section::Text {
            label: "Balance".into(),
            value: fmt(s.available),
        },
        Section::Block {
            label: "Breakdown".into(),
            body: vec![format!("cash {} · voucher {}", fmt(s.cash), fmt(s.voucher))],
        },
    ])
}

fn grok_sections(s: &crate::usage::GrokSnapshot) -> SectionBuilder {
    SectionBuilder::new(vec![
        Section::Title {
            left: "Grok (xAI)".into(),
            right: None,
        },
        Section::Spacer,
        Section::Text {
            label: "Prepaid balance".into(),
            value: usd(s.balance),
        },
    ])
}

fn supergrok_sections(s: &crate::usage::SuperGrokSnapshot, now: DateTime<Utc>) -> SectionBuilder {
    let pct = s.weekly_pct;
    let mut v = SectionBuilder::new(vec![
        Section::Title {
            left: s.plan.clone(),
            right: None,
        },
        Section::Spacer,
    ]);
    v.push_metric(
        Section::Metric {
            label: format!("{} Build credits", s.period.label()),
            pct: pct.clamp(0, 100) as u16,
            severity: severity_for(pct),
            value_label: format!("{pct}%"),
            footnote: String::new(),
        },
        s.reset_at,
    );
    if let Some(bal) = s.prepaid_balance {
        v.push(Section::Spacer);
        v.push(Section::Text {
            label: "Prepaid API".into(),
            value: usd(bal),
        });
    }
    push_reset_credits(&mut v, &s.reset_credits, now);
    v
}

fn push_reset_credits(
    v: &mut SectionBuilder,
    credits: &crate::usage::ResetCredits,
    now: DateTime<Utc>,
) {
    if credits.is_empty() {
        return;
    }
    v.push(Section::Spacer);
    v.push(Section::Block {
        label: "Reset credits".into(),
        body: reset_credit_lines(credits, now),
    });
}

fn deepseek_sections(s: &crate::usage::DeepseekSnapshot) -> SectionBuilder {
    let currency = &s.currency;
    let fmt = |v: f64| money(v, currency);
    let avail = if s.is_available {
        "available"
    } else {
        "unavailable"
    };
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: "DeepSeek".into(),
        right: None,
    }]);
    v.push(Section::Spacer);
    v.push(Section::Text {
        label: "Balance".into(),
        value: fmt(s.balance),
    });
    v.push(Section::Block {
        label: "Breakdown".into(),
        body: vec![format!(
            "granted {} · topped-up {}",
            fmt(s.granted),
            fmt(s.topped_up)
        )],
    });
    v.push(Section::Spacer);
    v.push(Section::Block {
        label: "API".into(),
        body: vec![avail.into()],
    });
    v
}

/// Kimi reports each quota as used/limit against a limit of 100, so the pair
/// is the percentage in longhand. Projecting both onto a `UsageWindow` lets
/// the shared `push_window` draw them, which is what keeps the row identical
/// to every other vendor's instead of a hand-rolled near-copy.
fn kimi_sections(s: &crate::usage::KimiSnapshot, now: DateTime<Utc>, tol: u32) -> SectionBuilder {
    use crate::kimi::vendor::{ROLLING_WINDOW, WEEKLY_WINDOW};

    let plan = s.plan.as_deref().unwrap_or("Kimi");
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: plan.into(),
        right: None,
    }]);
    let window = |pct, resets_at, window_duration| crate::usage::UsageWindow {
        utilization_pct: pct,
        resets_at,
        window_duration,
    };

    if s.window_limit > 0 {
        let w = window(s.window_pct(), s.window_reset_at, ROLLING_WINDOW);
        push_window(&mut v, "Rolling window (5h)", &w, now, tol, false);
    }
    let w = window(s.weekly_pct(), s.weekly_reset_at, WEEKLY_WINDOW);
    push_window(&mut v, "Weekly quota", &w, now, tol, false);

    v
}

fn push_window(
    sections: &mut SectionBuilder,
    label: &str,
    w: &crate::usage::UsageWindow,
    now: DateTime<Utc>,
    tol: u32,
    show_pacing: bool,
) {
    let pct = w.utilization_pct.clamp(0, 100) as u16;
    let reset_text = countdown::format(w.resets_at, now);
    let footnote = if show_pacing {
        let p = pacing::calc(w.utilization_pct, w.resets_at, now, w.window_duration, tol);
        format!(
            "Resets in {} · {}% elapsed · {}",
            reset_text, p.elapsed_pct, p.point_label
        )
    } else {
        format!("Resets in {}", reset_text)
    };
    sections.push(Section::Spacer);
    sections.push_metric(
        Section::Metric {
            label: label.into(),
            pct,
            severity: severity_for(pct as i32),
            value_label: format!("{pct}%"),
            footnote,
        },
        w.resets_at,
    );
}

/// Render the given sections into `area`. Lays them out vertically; metric
/// rows take 2 lines (label+gauge / footnote), text and spacer rows take 1.
///
/// The trailing "Updated …" footer is detected (the last `Text` section)
/// and pinned to the bottom of the area, with the slack absorbed *between*
/// content and footer. This way shorter vendor panels (OpenRouter, Z.AI)
/// don't leave a giant gap below the footer.
pub fn render(f: &mut Frame, area: Rect, theme: &Theme, sections: &[Section]) {
    if sections.is_empty() {
        return;
    }
    let bubble = bubble_theme(theme);
    // Heuristic: if the last section is a Text starting with "  Updated",
    // pin it to the bottom. Otherwise just lay everything out top-down.
    let pin_last =
        matches!(sections.last(), Some(Section::Text { value, .. }) if value.contains("Updated"));

    let body_end = if pin_last {
        sections.len() - 1
    } else {
        sections.len()
    };
    let mut constraints: Vec<Constraint> =
        sections[..body_end].iter().map(section_height).collect();

    if pin_last {
        constraints.push(Constraint::Min(0)); // slack between body and footer
        constraints.push(section_height(sections.last().unwrap()));
    } else {
        constraints.push(Constraint::Min(0));
    }

    let chunks = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints(constraints)
        .split(area);

    for (i, s) in sections[..body_end].iter().enumerate() {
        render_section(f, chunks[i], theme, &bubble, s);
    }
    if pin_last {
        render_section(
            f,
            chunks[chunks.len() - 1],
            theme,
            &bubble,
            sections.last().unwrap(),
        );
    }
}

fn section_height(s: &Section) -> Constraint {
    match s {
        Section::Title { .. } => Constraint::Length(2),
        Section::Metric { .. } => Constraint::Length(3),
        Section::Text { .. } => Constraint::Length(1),
        Section::Block { body, .. } => Constraint::Length(1 + body.len() as u16),
        Section::Spacer => Constraint::Length(1),
    }
}

fn render_section(f: &mut Frame, area: Rect, theme: &Theme, bubble: &BubbleTheme, s: &Section) {
    match s {
        Section::Title { left, right } => {
            // Left: bold accent-colored plan/vendor label. Right: dim-styled
            // "Updated HH:MM:SS" pinned to the right edge of the title row.
            let left_line = Line::from(Span::styled(
                format!("  {} {left}", bubble.symbols.selected),
                bubble.title,
            ));
            f.render_widget(Paragraph::new(left_line), area);
            if let Some(rt) = right {
                let right_line =
                    Line::from(Span::styled(format!("{rt}  "), bubble.muted)).right_aligned();
                f.render_widget(Paragraph::new(right_line), area);
            }
        }
        Section::Metric {
            label,
            pct,
            severity,
            value_label,
            footnote,
        } => render_metric(
            f,
            area,
            theme,
            bubble,
            label,
            *pct,
            *severity,
            value_label,
            footnote,
        ),
        Section::Text { label, value } => {
            if label.is_empty() && value.contains("Loading") {
                render_loading(f, area, bubble);
                return;
            }
            if label == "Error" {
                let line = Line::from(vec![
                    bubble.error(format!("  {} ", bubble.symbols.cross)),
                    Span::styled(value.clone(), bubble.error.add_modifier(Modifier::BOLD)),
                ]);
                f.render_widget(Paragraph::new(line), area);
                return;
            }
            let mut spans = Vec::new();
            if !label.is_empty() {
                spans.push(Span::styled(
                    format!("  {label}  "),
                    bubble.text.add_modifier(Modifier::BOLD),
                ));
            }
            spans.push(Span::styled(value.clone(), bubble.muted));
            f.render_widget(Paragraph::new(Line::from(spans)), area);
        }
        Section::Block { label, body } => render_block(f, area, bubble, label, body),
        Section::Spacer => {}
    }
}

fn render_loading(f: &mut Frame, area: Rect, bubble: &BubbleTheme) {
    let frames = SpinnerFrames::DOTS;
    let frame_count = frames.frames().len().max(1);
    let frame = chrono::Utc::now().timestamp_millis().unsigned_abs() as usize / 120;
    let mut spinner = Spinner::new()
        .frames(frames)
        .label("Fetching usage data")
        .theme(*bubble);
    for _ in 0..(frame % frame_count) {
        spinner.tick();
    }
    f.render_widget(&spinner, area);
}

#[allow(clippy::too_many_arguments)]
fn render_metric(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    bubble: &BubbleTheme,
    label: &str,
    pct: u16,
    severity: PaceSeverity,
    value_label: &str,
    footnote: &str,
) {
    let bar_color = severity_color(theme, bubble, severity);
    let bar_empty = color(&theme.bar_empty).unwrap_or(bubble.palette.selected_background);

    let inner = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    // Row 1: label
    let label_line = Line::from(Span::styled(
        format!("  {label}"),
        bubble.text.add_modifier(Modifier::BOLD),
    ));
    f.render_widget(Paragraph::new(label_line), inner[0]);

    // Row 2: gauge spanning most of the width + value annotation on the right
    let row = inner[1];
    let value_w = value_label.chars().count() as u16 + 2;
    let gauge_area = Rect {
        x: row.x + 2,
        y: row.y,
        width: row.width.saturating_sub(value_w + 4),
        height: 1,
    };
    let value_area = Rect {
        x: gauge_area.x + gauge_area.width + 1,
        y: row.y,
        width: value_w,
        height: 1,
    };
    let progress_theme = progress_theme(*bubble, bar_color, bar_empty);
    let progress = Progress::from_percent(pct)
        .theme(progress_theme)
        .show_percentage(false);
    f.render_widget(&progress, gauge_area);
    let value = Paragraph::new(Line::from(Span::styled(
        value_label.to_string(),
        Style::default().fg(bar_color).add_modifier(Modifier::BOLD),
    )));
    f.render_widget(value, value_area);

    // Row 3: footnote (dim)
    let foot = Line::from(Span::styled(format!("    {footnote}"), bubble.muted));
    f.render_widget(Paragraph::new(foot), inner[2]);
}

fn render_block(f: &mut Frame, area: Rect, bubble: &BubbleTheme, label: &str, body: &[String]) {
    let mut lines = vec![Line::from(Span::styled(
        format!("  {label}"),
        bubble.text.add_modifier(Modifier::BOLD),
    ))];
    for b in body {
        lines.push(Line::from(Span::styled(format!("    {b}"), bubble.muted)));
    }
    f.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{
        AnthropicSnapshot, Cents, ExtraUsage, KimiSnapshot, OpenAiCredits, OpenAiSnapshot,
        OpenAiSource, OpenRouterSnapshot, ResetCredit, ResetCredits, UsageWindow, ZaiSnapshot,
    };
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap()
    }

    fn ready(snapshot: VendorSnapshot) -> TabState {
        TabState::Ready(Box::new(crate::tui::app::ReadyTab {
            snapshot,
            stale: false,
            last_error: None,
            fetched_at: Some(now() - chrono::Duration::seconds(15)),
        }))
    }

    #[test]
    fn copilot_sections_carry_quota_reset_metadata() {
        let reset_at = now() + chrono::Duration::days(4);
        let snapshot = VendorSnapshot::Copilot(crate::copilot::types::Snapshot {
            plan: "Pro".into(),
            premium: Some(crate::copilot::types::Quota {
                percent_remaining: 25,
                entitlement: Some(300),
                remaining: Some(75),
                unlimited: false,
            }),
            chat: None,
            completions: None,
            reset_at: Some(reset_at),
        });
        let sections = sections_with_metadata_for(&ready(snapshot), now(), 5);
        assert!(matches!(
            &sections[0].section,
            Section::Title { left, .. } if left == "GitHub Copilot Pro"
        ));
        let metric = sections
            .iter()
            .find(|projection| matches!(&projection.section, Section::Metric { .. }))
            .expect("premium metric");
        assert_eq!(metric.reset_at, Some(reset_at));
        assert!(matches!(
            &metric.section,
            Section::Metric { label, pct, value_label, footnote, .. }
                if label == "Premium requests"
                    && *pct == 75
                    && value_label == "75%"
                    && footnote == "225 of 300 used"
        ));
    }

    #[test]
    fn anthropic_sections_include_all_three_windows_when_present() {
        let snap = AnthropicSnapshot {
            plan: "Max 20x".into(),
            session: UsageWindow {
                utilization_pct: 60,
                resets_at: Some(now() + chrono::Duration::hours(1)),
                window_duration: chrono::Duration::hours(5),
            },
            weekly: UsageWindow {
                utilization_pct: 30,
                resets_at: Some(now() + chrono::Duration::days(3)),
                window_duration: chrono::Duration::days(7),
            },
            sonnet: Some(UsageWindow {
                utilization_pct: 5,
                resets_at: Some(now() + chrono::Duration::hours(2)),
                window_duration: chrono::Duration::days(7),
            }),
            scoped: vec![],
            extra: Some(ExtraUsage {
                limit: Some(Cents(5000)),
                spent: Cents(250),
                currency: None,
                decimal_places: Some(2),
            }),
        };
        let sections = sections_for(&ready(VendorSnapshot::Anthropic(snap)), now(), 5);
        // Title (carries "Updated …" inline now) + 4 metrics (3 windows +
        // extra) each preceded by a Spacer. 1 + 4*2 = 9 sections.
        assert_eq!(sections.len(), 9);
        assert!(matches!(sections[0], Section::Title { .. }));
        // Title's right-aligned slot should carry the timestamp.
        if let Section::Title { right, .. } = &sections[0] {
            assert!(right.as_deref().is_some_and(|r| r.starts_with("Updated ")));
        } else {
            panic!("expected first section to be Title");
        }
        let metric_count = sections
            .iter()
            .filter(|s| matches!(s, Section::Metric { .. }))
            .count();
        assert_eq!(metric_count, 4);
    }

    #[test]
    fn anthropic_uncapped_extra_shows_spend_without_a_denominator() {
        // The #30 shape: `monthly_limit: null` (Pro). The panel must show the
        // spend alone — not "of $0.00", not an invented percentage.
        let snap = AnthropicSnapshot {
            plan: "Pro".into(),
            session: UsageWindow {
                utilization_pct: 10,
                resets_at: None,
                window_duration: chrono::Duration::hours(5),
            },
            weekly: UsageWindow {
                utilization_pct: 20,
                resets_at: None,
                window_duration: chrono::Duration::days(7),
            },
            sonnet: None,
            scoped: vec![],
            extra: Some(ExtraUsage {
                limit: None,
                spent: Cents(14157),
                currency: Some("BRL".into()),
                decimal_places: Some(2),
            }),
        };
        let sections = sections_for(&ready(VendorSnapshot::Anthropic(snap)), now(), 5);
        let extra = sections
            .iter()
            .find_map(|s| match s {
                Section::Metric {
                    label,
                    pct,
                    value_label,
                    footnote,
                    ..
                } if label == "Extra usage" => Some((*pct, value_label.clone(), footnote.clone())),
                _ => None,
            })
            .expect("uncapped extra usage must still render a section");
        assert_eq!(extra.0, 0);
        // Non-vacuous currency pin: fmt_dollars would say "$141.57" here.
        assert_eq!(extra.1, "R$141.57");
        assert!(
            !extra.1.contains(" of "),
            "no denominator to show: {}",
            extra.1
        );
        assert_eq!(extra.2, "no monthly limit reported");
    }

    #[test]
    fn anthropic_omits_sonnet_and_extra_when_absent() {
        let snap = AnthropicSnapshot {
            plan: "Pro".into(),
            session: UsageWindow {
                utilization_pct: 10,
                resets_at: None,
                window_duration: chrono::Duration::hours(5),
            },
            weekly: UsageWindow {
                utilization_pct: 5,
                resets_at: None,
                window_duration: chrono::Duration::days(7),
            },
            sonnet: None,
            scoped: vec![],
            extra: None,
        };
        let sections = sections_for(&ready(VendorSnapshot::Anthropic(snap)), now(), 5);
        let metric_count = sections
            .iter()
            .filter(|s| matches!(s, Section::Metric { .. }))
            .count();
        assert_eq!(metric_count, 2);
    }

    #[test]
    fn openrouter_always_has_balance_metric_and_period_block() {
        let snap = OpenRouterSnapshot {
            label: "OR".into(),
            total_credits: 100.0,
            total_usage: 25.0,
            usage_daily: 1.0,
            usage_weekly: 5.0,
            usage_monthly: 25.0,
            is_free_tier: false,
            limit: None,
            limit_remaining: None,
        };
        let sections = sections_for(&ready(VendorSnapshot::Openrouter(snap)), now(), 5);
        assert!(matches!(sections[0], Section::Title { .. }));
        assert!(
            sections
                .iter()
                .any(|s| matches!(s, Section::Metric { label, .. } if label == "Credit balance"))
        );
        assert!(
            sections
                .iter()
                .any(|s| matches!(s, Section::Block { label, .. } if label == "Usage by period"))
        );
    }

    /// #118 reached every frontend, not just Waybar: the panel row is what the
    /// Omarchy, GNOME and KDE plugins colour and label from, so the debt has to
    /// survive the projection with its sign and its severity intact.
    #[test]
    fn openrouter_debt_reaches_the_panel_row_red_and_signed() {
        let snap = OpenRouterSnapshot {
            label: "OR".into(),
            total_credits: 0.0,
            total_usage: 5.71,
            usage_daily: 1.0,
            usage_weekly: 5.0,
            usage_monthly: 5.71,
            is_free_tier: false,
            limit: None,
            limit_remaining: None,
        };
        let sections = sections_for(&ready(VendorSnapshot::Openrouter(snap.clone())), now(), 5);
        let metric = sections
            .iter()
            .find_map(|s| match s {
                Section::Metric {
                    label,
                    value_label,
                    severity,
                    footnote,
                    ..
                } if label == "Credit balance" => Some((value_label, severity, footnote)),
                _ => None,
            })
            .expect("no credit balance metric");
        assert_eq!(metric.0, "-$5.71");
        assert_eq!(*metric.1, PaceSeverity::Critical);
        assert_eq!(metric.2, "$5.71 of $0.00 used (0%)");

        // ...and the same number in the dense Overview list.
        let (_, cells) = compact_cells(&VendorSnapshot::Openrouter(snap));
        assert_eq!(cells[0].0, "-$5.71");
    }

    /// The panels express pace as a footnote on the row; the arrow is the
    /// widget's idiom and the bar tick the menu bar's. All three Z.AI windows
    /// report a duration and a reset, so all three carry one.
    #[test]
    fn zai_windows_are_paced_like_every_other_percentage_vendor() {
        let window = |pct: i32, hours: i64, span: chrono::Duration| crate::usage::UsageWindow {
            utilization_pct: pct,
            resets_at: Some(now() + chrono::Duration::hours(hours)),
            window_duration: span,
        };
        let snap = ZaiSnapshot {
            plan: "GLM Coding Pro".into(),
            session: Some(window(40, 2, chrono::Duration::hours(5))),
            weekly: Some(window(60, 48, chrono::Duration::days(7))),
            mcp: Some(window(10, 200, chrono::Duration::days(30))),
        };

        let footnotes: Vec<String> = sections_for(&ready(VendorSnapshot::Zai(snap)), now(), 5)
            .into_iter()
            .filter_map(|section| match section {
                Section::Metric { footnote, .. } => Some(footnote),
                _ => None,
            })
            .collect();

        assert_eq!(footnotes.len(), 3, "{footnotes:?}");
        for footnote in &footnotes {
            assert!(footnote.contains("% elapsed"), "{footnote}");
        }
        // 40% used with 60% of a 5h window gone: behind pace, not ahead.
        assert_eq!(
            footnotes[0], "Resets in 2h 00m · 60% elapsed · 20pts under",
            "{footnotes:?}"
        );
    }

    #[test]
    fn zai_no_windows_renders_message() {
        let snap = ZaiSnapshot {
            plan: "GLM".into(),
            session: None,
            weekly: None,
            mcp: None,
        };
        let sections = sections_for(&ready(VendorSnapshot::Zai(snap)), now(), 5);
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { value, .. } if value.contains("no usage windows reported")
        )));
    }

    #[test]
    fn openai_no_windows_renders_message() {
        let snap = OpenAiSnapshot {
            plan: "ChatGPT Plus".into(),
            session: None,
            weekly: None,
            code_review: None,
            additional_limits: Vec::new(),
            unavailable_models: Vec::new(),
            credits: None,
            reset_credits: ResetCredits::default(),
            source: OpenAiSource::CodexOauth,
        };
        let sections = sections_for(&ready(VendorSnapshot::Openai(snap)), now(), 5);
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { value, .. } if value.contains("no usage windows reported")
        )));
    }

    #[test]
    fn loading_state_yields_loading_section() {
        let sections = sections_for(&TabState::Loading, now(), 5);
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { value, .. } if value.contains("Loading")
        )));
    }

    #[test]
    fn error_state_includes_retry_hint() {
        let sections = sections_for(&TabState::Error("token expired".into()), now(), 5);
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { value, .. } if value.contains("token expired")
        )));
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { value, .. } if value.contains("`r` to retry")
        )));
    }

    #[test]
    fn openai_with_credits_renders_block() {
        let snap = OpenAiSnapshot {
            plan: "ChatGPT Plus".into(),
            session: Some(UsageWindow {
                utilization_pct: 1,
                resets_at: None,
                window_duration: chrono::Duration::hours(5),
            }),
            weekly: Some(UsageWindow {
                utilization_pct: 0,
                resets_at: None,
                window_duration: chrono::Duration::days(7),
            }),
            code_review: None,
            additional_limits: Vec::new(),
            unavailable_models: Vec::new(),
            credits: Some(OpenAiCredits {
                balance: "$5.00".into(),
                has_credits: true,
                unlimited: false,
                approx_local_messages: Some((100, 200)),
                approx_cloud_messages: Some((30, 50)),
            }),
            reset_credits: ResetCredits::default(),
            source: OpenAiSource::CodexOauth,
        };
        let sections = sections_for(&ready(VendorSnapshot::Openai(snap)), now(), 5);
        assert!(
            sections
                .iter()
                .any(|s| matches!(s, Section::Block { label, .. } if label == "Credits"))
        );
    }

    #[test]
    fn openai_weekly_only_omits_session_section() {
        let snap = OpenAiSnapshot {
            plan: "ChatGPT Prolite".into(),
            session: None,
            weekly: Some(UsageWindow {
                utilization_pct: 66,
                resets_at: None,
                window_duration: chrono::Duration::days(7),
            }),
            code_review: None,
            additional_limits: Vec::new(),
            unavailable_models: Vec::new(),
            credits: None,
            reset_credits: ResetCredits::default(),
            source: OpenAiSource::CodexOauth,
        };
        let sections = sections_for(&ready(VendorSnapshot::Openai(snap)), now(), 5);
        assert!(sections.iter().any(|section| matches!(
            section,
            Section::Metric { label, .. } if label == "Codex weekly"
        )));
        assert!(!sections.iter().any(|section| matches!(
            section,
            Section::Metric { label, .. } if label == "Codex 5h"
        )));
    }

    /// Both vendors reach every frontend through these sections — the TUI
    /// panel, `usage --json`, and from there the Omarchy, GNOME and KDE
    /// surfaces. One row, one wording, whichever provider banked the reset.
    #[test]
    fn banked_resets_reach_the_panel_for_both_providers() {
        let now = now();
        let credits = ResetCredits {
            available: 2,
            credits: vec![
                ResetCredit {
                    title: Some("Full reset (Weekly + 5 hr)".into()),
                    expires_at: Some(now + chrono::Duration::days(13)),
                },
                ResetCredit {
                    title: Some("Full reset (Weekly + 5 hr)".into()),
                    expires_at: Some(now + chrono::Duration::days(13) + chrono::Duration::hours(6)),
                },
            ],
        };
        let codex = OpenAiSnapshot {
            plan: "ChatGPT Plus".into(),
            session: None,
            weekly: None,
            code_review: None,
            additional_limits: Vec::new(),
            unavailable_models: Vec::new(),
            credits: None,
            reset_credits: credits.clone(),
            source: OpenAiSource::CodexOauth,
        };
        let supergrok = crate::usage::SuperGrokSnapshot {
            plan: "SuperGrok".into(),
            account: "scope".into(),
            weekly_pct: 30,
            period: crate::usage::SuperGrokPeriod::Weekly,
            reset_at: Some(now + chrono::Duration::days(3)),
            prepaid_balance: None,
            reset_credits: credits,
        };

        for snapshot in [
            VendorSnapshot::Openai(codex),
            VendorSnapshot::SuperGrok(supergrok),
        ] {
            let sections = sections_for(&ready(snapshot), now, 5);
            let body = sections.iter().find_map(|section| match section {
                Section::Block { label, body } if label == "Reset credits" => Some(body.clone()),
                _ => None,
            });
            let body = body.expect("reset credits block");
            assert_eq!(body.len(), 2, "{body:?}");
            assert!(
                body.iter()
                    .all(|line| line.contains("Full reset (Weekly + 5 hr)")),
                "{body:?}"
            );
        }
    }

    /// The row is absent, not zeroed: an account that has never earned a reset
    /// should not carry a permanent "0 resets available" line.
    #[test]
    fn a_provider_with_no_banked_resets_shows_no_reset_row() {
        let snap = OpenAiSnapshot {
            plan: "ChatGPT Plus".into(),
            session: None,
            weekly: None,
            code_review: None,
            additional_limits: Vec::new(),
            unavailable_models: Vec::new(),
            credits: None,
            reset_credits: ResetCredits::default(),
            source: OpenAiSource::CodexOauth,
        };
        let sections = sections_for(&ready(VendorSnapshot::Openai(snap)), now(), 5);
        assert!(!sections.iter().any(|section| matches!(
            section,
            Section::Block { label, .. } if label == "Reset credits"
        )));
    }

    #[test]
    fn supergrok_does_not_repeat_the_window_reset_as_its_own_section() {
        let now = now();
        let snap = crate::usage::SuperGrokSnapshot {
            plan: "SuperGrok".into(),
            account: "scope".into(),
            weekly_pct: 0,
            period: crate::usage::SuperGrokPeriod::Weekly,
            reset_at: Some(now + chrono::Duration::days(6)),
            prepaid_balance: Some(0.0),
            reset_credits: ResetCredits::default(),
        };
        let sections = sections_for(&ready(VendorSnapshot::SuperGrok(snap)), now, 5);
        assert!(!sections.iter().any(|section| matches!(
            section,
            Section::Text { label, .. } if label == "Resets"
        )));
        assert!(sections.iter().any(|section| matches!(
            section,
            Section::Text { label, .. } if label == "Prepaid API"
        )));
    }

    #[test]
    fn kimi_sections_include_weekly_and_window_with_used_over_limit() {
        let now = now();
        let snap = KimiSnapshot {
            plan: Some("LEVEL_INTERMEDIATE".into()),
            weekly_limit: 100,
            weekly_used: 26,
            weekly_remaining: 74,
            weekly_reset_at: Some(now + chrono::Duration::days(4)),
            window_limit: 100,
            window_used: 15,
            window_remaining: 85,
            window_reset_at: Some(now + chrono::Duration::hours(2)),
        };
        let sections = sections_for(&ready(VendorSnapshot::Kimi(snap)), now, 5);
        let metrics: Vec<_> = sections
            .iter()
            .filter(|s| matches!(s, Section::Metric { .. }))
            .collect();
        assert_eq!(metrics.len(), 2);
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Metric { label, .. } if label == "Weekly quota"
        )));
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Metric { label, .. } if label == "Rolling window (5h)"
        )));

        let find_footnote = |label: &str| -> (String, String) {
            sections
                .iter()
                .find_map(|s| match s {
                    Section::Metric {
                        label: l,
                        value_label,
                        footnote,
                        ..
                    } if l == label => Some((value_label.clone(), footnote.clone())),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing metric {label}"))
        };

        // The bar carries the percentage, so the footnote is the plain
        // `Resets in …` every other window row shows — not the counters, which
        // against Kimi's limit of 100 only restate the percentage.
        let (weekly_value, weekly_footnote) = find_footnote("Weekly quota");
        assert_eq!(weekly_value, "26%");
        assert_eq!(weekly_footnote, "Resets in 4d 0h");

        let (window_value, window_footnote) = find_footnote("Rolling window (5h)");
        assert_eq!(window_value, "15%");
        assert_eq!(window_footnote, "Resets in 2h 00m");
    }

    /// Every vendor holding both a short and a long window opens on the short
    /// one — Claude's `Session (5h)`, Codex's `Codex 5h`, GLM's `Session (5h)`,
    /// OpenCode Go's `Rolling`. Kimi's rolling bucket is that window, so it
    /// leads both projections this module feeds: the section list the Quattro
    /// panel and the KDE plasmoid render in order, and the Overview's compact
    /// cells.
    #[test]
    fn kimi_leads_with_the_rolling_window_like_every_other_two_window_vendor() {
        let now = now();
        let snap = KimiSnapshot {
            plan: Some("LEVEL_INTERMEDIATE".into()),
            weekly_limit: 100,
            weekly_used: 26,
            weekly_remaining: 74,
            weekly_reset_at: Some(now + chrono::Duration::days(4)),
            window_limit: 100,
            window_used: 15,
            window_remaining: 85,
            window_reset_at: Some(now + chrono::Duration::hours(2)),
        };
        let sections = sections_for(&ready(VendorSnapshot::Kimi(snap.clone())), now, 5);
        let labels: Vec<&str> = sections
            .iter()
            .filter_map(|s| match s {
                Section::Metric { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(labels, ["Rolling window (5h)", "Weekly quota"]);

        let (_, cells) = compact_cells(&VendorSnapshot::Kimi(snap));
        let texts: Vec<&str> = cells.iter().map(|(text, _)| text.as_str()).collect();
        assert_eq!(texts, ["5h 15%", "wk 26%"]);
    }

    #[test]
    fn kimi_sections_omit_window_when_limit_zero() {
        let snap = KimiSnapshot {
            plan: None,
            weekly_limit: 100,
            weekly_used: 10,
            weekly_remaining: 90,
            weekly_reset_at: None,
            window_limit: 0,
            window_used: 0,
            window_remaining: 0,
            window_reset_at: None,
        };
        let sections = sections_for(&ready(VendorSnapshot::Kimi(snap)), now(), 5);
        let metric_count = sections
            .iter()
            .filter(|s| matches!(s, Section::Metric { .. }))
            .count();
        assert_eq!(metric_count, 1);
    }

    fn cursor_snap() -> crate::usage::CursorSnapshot {
        crate::usage::CursorSnapshot {
            plan: "Ultra".into(),
            auto_pct: 98,
            api_pct: 100,
            total_pct: 99,
            unlimited: false,
            on_demand_enabled: false,
            reset_at: Some(now() + chrono::Duration::days(9)),
        }
    }

    #[test]
    fn compact_cells_flatten_key_metrics_for_the_overview() {
        // Percent vendor (Cursor): plan + two colored pool cells.
        let (plan, cells) = compact_cells(&VendorSnapshot::Cursor(cursor_snap()));
        assert_eq!(plan, "Ultra");
        assert_eq!(cells[0].0, "auto 98%");
        assert_eq!(cells[1].0, "premium 100%");
        assert_eq!(cells[1].1, PaceSeverity::Critical); // 100% is critical

        // Balance vendor (Kilo): no plan, a single money cell, calm severity.
        let (plan, cells) = compact_cells(&VendorSnapshot::Kilo(crate::usage::KiloSnapshot {
            label: "Kilo".into(),
            balance: 8.42,
        }));
        assert!(plan.is_empty());
        assert_eq!(cells, vec![("$8.42".to_string(), PaceSeverity::Low)]);
    }

    #[test]
    fn terminal_controls_are_removed_from_detail_and_overview_fields() {
        let error = TabState::Error("bad\x1b]52;c;Y2FuYXJ5\x07 value".into());
        let sections = sections_for(&error, now(), 5);
        assert!(matches!(
            &sections[1],
            Section::Text { value, .. }
                if value == "bad]52;c;Y2FuYXJ5 value"
                    && !value.chars().any(|ch| ch.is_control())
        ));

        let mut snapshot = cursor_snap();
        snapshot.plan = "Ultra\x1b[2J\x07".into();
        let (plan, _) = compact_cells(&VendorSnapshot::Cursor(snapshot));
        assert_eq!(plan, "Ultra[2J");
        assert!(!plan.chars().any(char::is_control));
    }

    #[test]
    fn headline_pct_is_the_worst_window_or_combined_total() {
        // Cursor: the combined total, not the worse pool (mirrors the menu bar).
        assert_eq!(
            headline_pct(&VendorSnapshot::Cursor(cursor_snap())),
            Some(99)
        );

        // Balance-only vendors have no meaningful percentage → no bar.
        let kilo = VendorSnapshot::Kilo(crate::usage::KiloSnapshot {
            label: "Kilo".into(),
            balance: 8.42,
        });
        assert_eq!(headline_pct(&kilo), None);
    }

    #[test]
    fn cursor_sections_show_both_pools_and_reset() {
        let sections = sections_for(&ready(VendorSnapshot::Cursor(cursor_snap())), now(), 5);
        let metrics: Vec<_> = sections
            .iter()
            .filter_map(|s| match s {
                Section::Metric {
                    label, value_label, ..
                } => Some((label.clone(), value_label.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(metrics.len(), 2, "two pools");
        assert!(
            metrics
                .iter()
                .any(|(l, v)| l == "Cursor Models" && v == "98%")
        );
        assert!(
            metrics
                .iter()
                .any(|(l, v)| l == "Other Models" && v == "100%")
        );
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { label, value } if label == "Resets" && value.contains("9d")
        )));
    }

    #[test]
    fn cursor_unlimited_plan_shows_no_pool_bars() {
        let mut snap = cursor_snap();
        snap.unlimited = true;
        let sections = sections_for(&ready(VendorSnapshot::Cursor(snap)), now(), 5);
        let metric_count = sections
            .iter()
            .filter(|s| matches!(s, Section::Metric { .. }))
            .count();
        assert_eq!(metric_count, 0);
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { value, .. } if value.contains("Unlimited")
        )));
    }

    fn kiro_snap() -> crate::usage::KiroSnapshot {
        crate::usage::KiroSnapshot {
            plan: "KIRO POWER".into(),
            used: 9943.38,
            limit: 10000.0,
            reset_at: Some(now() + chrono::Duration::days(1)),
        }
    }

    #[test]
    fn kiro_compact_cell_shows_the_credit_percentage() {
        let (plan, cells) = compact_cells(&VendorSnapshot::Kiro(kiro_snap()));
        assert_eq!(plan, "KIRO POWER");
        assert_eq!(
            cells,
            vec![("credits 99%".to_string(), PaceSeverity::Critical)]
        );
    }

    #[test]
    fn kiro_headline_pct_is_the_credit_percentage() {
        assert_eq!(headline_pct(&VendorSnapshot::Kiro(kiro_snap())), Some(99));
    }

    #[test]
    fn kiro_sections_show_the_credit_metric_and_reset() {
        let sections = sections_for(&ready(VendorSnapshot::Kiro(kiro_snap())), now(), 5);
        let metrics: Vec<_> = sections
            .iter()
            .filter_map(|s| match s {
                Section::Metric {
                    label, value_label, ..
                } => Some((label.clone(), value_label.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(metrics, vec![("Credits".to_string(), "99%".to_string())]);
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { label, value } if label == "Resets" && value.contains("1d")
        )));
    }

    #[test]
    fn schema_drift_and_generic_code_zero_diagnostics_are_visible_without_http_labels() {
        let snap = KimiSnapshot {
            plan: None,
            weekly_limit: 100,
            weekly_used: 10,
            weekly_remaining: 90,
            weekly_reset_at: None,
            window_limit: 0,
            window_used: 0,
            window_remaining: 0,
            window_reset_at: None,
        };
        let mut schema = ready(VendorSnapshot::Kimi(snap.clone()));
        let TabState::Ready(tab) = &mut schema else {
            unreachable!()
        };
        tab.last_error = Some((0, crate::kimi::fetch::SCHEMA_DRIFT_MESSAGE.into()));
        let schema_sections = sections_for(&schema, now(), 5);
        assert!(schema_sections.iter().any(|section| matches!(
            section,
            Section::Text { label, value } if label == "Kimi API schema drift" && value.is_empty()
        )));

        let mut generic = ready(VendorSnapshot::Kimi(snap));
        let TabState::Ready(tab) = &mut generic else {
            unreachable!()
        };
        tab.last_error = Some((0, "cache lock unavailable".into()));
        let generic_sections = sections_for(&generic, now(), 5);
        assert!(generic_sections.iter().any(|section| matches!(
            section,
            Section::Text { label, value } if label == "Warning" && value == "cache lock unavailable"
        )));
        assert!(!generic_sections.iter().any(|section| matches!(
            section,
            Section::Text { label, .. } if label.starts_with("HTTP")
        )));

        let http = warning_label(
            &VendorSnapshot::Kimi(KimiSnapshot {
                plan: None,
                weekly_limit: 0,
                weekly_used: 0,
                weekly_remaining: 0,
                weekly_reset_at: None,
                window_limit: 0,
                window_used: 0,
                window_remaining: 0,
                window_reset_at: None,
            }),
            &Some((503, "service unavailable".into())),
        );
        assert_eq!(
            http,
            Some(("HTTP 503".into(), "service unavailable".into()))
        );
    }
}
