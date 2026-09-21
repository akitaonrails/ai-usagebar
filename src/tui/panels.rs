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

use crate::balance::{self, DisplayPrefs, MetricHeadline};
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
    /// Full length of the metric's reset window, when it is known exactly
    /// (a rolling 5h/7d window). `None` for calendar periods and for quotas
    /// whose window length the vendor never states — a frontend can pace a
    /// metric only when this is `Some`.
    pub window: Option<chrono::Duration>,
    /// Named sub-group the metric belongs under (e.g. SuperGrok's product
    /// slices under `"Breakdown"`), so a frontend can draw it as a compact
    /// row beneath a heading instead of a peer of the overall meter. The TUI
    /// renders grouped metrics like any other; only the report carries this.
    pub group: Option<&'static str>,
    /// Which of the metric's two numbers a report-driven frontend puts on the
    /// bar. Every metric is a percentage unless its vendor says otherwise, so
    /// this is [`MetricHeadline::Percent`] by default — the bar draws it and
    /// leaves the money figure in the detail. A prepaid-balance vendor flips
    /// it, and that declaration is what the frontend reads instead of guessing
    /// from the label.
    pub headline: MetricHeadline,
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
                        window: None,
                        group: None,
                        headline: MetricHeadline::Percent,
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
            window: None,
            group: None,
            headline: MetricHeadline::Percent,
        });
    }

    /// A metric whose window length is not known exactly (a calendar month,
    /// a vendor-defined billing period, or no stated window at all).
    fn push_metric(&mut self, section: Section, reset_at: Option<DateTime<Utc>>) {
        assert!(matches!(section, Section::Metric { .. }));
        self.0.push(SectionProjection {
            section,
            reset_at,
            window: None,
            group: None,
            headline: MetricHeadline::Percent,
        });
    }

    /// A metric whose vendor chose which of its two numbers goes on the bar.
    /// Only a prepaid-balance vendor needs this; everything else is a
    /// percentage and uses [`SectionBuilder::push_metric`].
    fn push_metric_with_headline(&mut self, section: Section, headline: MetricHeadline) {
        assert!(matches!(section, Section::Metric { .. }));
        self.0.push(SectionProjection {
            section,
            reset_at: None,
            window: None,
            group: None,
            headline,
        });
    }

    /// A metric on a window of exactly `window` length, so a frontend can
    /// compute how far through the window `reset_at` sits.
    fn push_metric_in_window(
        &mut self,
        section: Section,
        reset_at: Option<DateTime<Utc>>,
        window: chrono::Duration,
    ) {
        assert!(matches!(section, Section::Metric { .. }));
        self.0.push(SectionProjection {
            section,
            reset_at,
            window: Some(window),
            group: None,
            headline: MetricHeadline::Percent,
        });
    }

    /// A metric that belongs to a named sub-group of the panel (SuperGrok's
    /// product slices under `"Breakdown"`). Grouped slices share the overall
    /// pool's window, so they carry no reset of their own; the group label is
    /// the only extra thing they assert.
    fn push_metric_in_group(&mut self, section: Section, group: &'static str) {
        assert!(matches!(section, Section::Metric { .. }));
        self.0.push(SectionProjection {
            section,
            reset_at: None,
            window: None,
            group: Some(group),
            headline: MetricHeadline::Percent,
        });
    }
}

/// The headline row of a prepaid-balance vendor: money remaining, drawn as a
/// consumed meter when something supplies a tank size and as a plain text row
/// when nothing does.
///
/// Every balance-only vendor gets the same shape from here, so the tank, the
/// severity floor and the detail line cannot drift apart between them.
///
/// `api_limit` is the denominator the vendor's own API states, if any; it wins
/// over the user's `display_limit`. `money_severity` is the vendor's existing
/// money-based tier, which stays the floor: a nearly empty wallet is still
/// critical however large the tank, and a mostly-spent tank is still high
/// however much money is nominally left.
fn push_balance_headline(
    v: &mut SectionBuilder,
    label: &str,
    remaining: f64,
    currency: &str,
    money_severity: PaceSeverity,
    api_limit: Option<f64>,
    prefs: DisplayPrefs,
) {
    let amount = money(remaining, currency);
    let Some(limit) = balance::denominator(api_limit, prefs.display_limit) else {
        // No denominator from either source: nothing to meter against, so the
        // row stays free text rather than becoming a fabricated 0% gauge.
        v.push(Section::Text {
            label: label.into(),
            value: amount,
        });
        return;
    };

    let pct = balance::consumed_pct(limit, remaining);
    let headline = balance::resolve_headline(prefs.headline, Some(limit));
    let of_limit = money(limit, currency);
    let (value_label, footnote) = match headline {
        MetricHeadline::Percent => (
            format!("{pct}%"),
            format!("{amount} of {of_limit} left ({pct}% used)"),
        ),
        MetricHeadline::Value => (
            amount.clone(),
            format!("{pct}% of {of_limit} used ({amount} left)"),
        ),
    };
    v.push_metric_with_headline(
        Section::Metric {
            label: label.into(),
            pct,
            severity: money_severity.max(severity_for(i32::from(pct))),
            value_label,
            footnote,
        },
        headline,
    );
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
        VendorSnapshot::Kimi(s) => {
            let mut cells = vec![pct("5h", s.window_pct())];
            if s.has_weekly {
                cells.push(pct("wk", s.weekly_pct()));
            }
            if let Some(monthly) = s.monthly_pct {
                cells.push(pct("mo", monthly));
            }
            (s.plan.clone().unwrap_or_default(), cells)
        }
        VendorSnapshot::Kilo(s) => (String::new(), vec![usd_cell(s.balance)]),
        VendorSnapshot::Novita(s) => (String::new(), vec![usd_cell(s.available)]),
        VendorSnapshot::Moonshot(s) => (String::new(), vec![money_cell(s.available, &s.currency)]),
        VendorSnapshot::Grok(s) => (String::new(), vec![usd_cell(s.balance)]),
        VendorSnapshot::SuperGrok(s) => (s.plan.clone(), vec![pct(s.period.short(), s.weekly_pct)]),
        VendorSnapshot::Grokbot(s) => {
            // No included allowance is a state, not a 0% — no meter cell.
            let cells = if s.has_included_allowance {
                vec![pct("wk", s.weekly_pct)]
            } else {
                vec![("—".into(), PaceSeverity::Low)]
            };
            (s.plan.clone(), cells)
        }
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
                ("monthly", s.monthly_window().as_ref()),
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
        VendorSnapshot::Ollama(s) => {
            let cells = [
                ("5h", s.session.as_ref()),
                ("wk", s.weekly.as_ref()),
                ("mo", s.monthly.as_ref()),
            ]
            .into_iter()
            .filter_map(|(label, window)| {
                window.map(|window| pct(label, window.utilization_pct.clamp(0, 100)))
            })
            .collect();
            (s.plan.clone(), cells)
        }
        VendorSnapshot::Custom(s) => (
            s.plan.clone().unwrap_or_default(),
            s.metrics
                .iter()
                .take(3)
                .map(|metric| pct(&metric.label, i32::from(metric.pct)))
                .collect(),
        ),
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
        VendorSnapshot::Kimi(s) => Some(s.worst_pct()),
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
        VendorSnapshot::Grokbot(s) => s.has_included_allowance.then_some(s.weekly_pct),
        VendorSnapshot::Ollama(s) => [
            s.session.as_ref().map(|w| w.utilization_pct),
            s.weekly.as_ref().map(|w| w.utilization_pct),
            s.monthly.as_ref().map(|w| w.utilization_pct),
        ]
        .into_iter()
        .flatten()
        .max(),
        VendorSnapshot::Custom(s) => s.metrics.first().map(|metric| i32::from(metric.pct)),
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
        TabState::Error { message: e, plan } => {
            let mut rows = Vec::new();
            if let Some(plan) = plan {
                rows.push(Section::Title {
                    left: plan.clone(),
                    right: None,
                });
            }
            rows.extend([
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
            ]);
            SectionBuilder::new(rows)
        }
        TabState::Ready(r) => {
            let snapshot = &r.snapshot;
            let last_error = &r.last_error;
            let prefs = r.display;
            let mut sections = match snapshot {
                VendorSnapshot::Anthropic(s) => anthropic_sections(s, now, pace_tolerance),
                VendorSnapshot::AnthropicApi(s) => anthropic_api_sections(s),
                VendorSnapshot::Openai(s) => openai_sections(s, now, pace_tolerance),
                VendorSnapshot::Copilot(s) => copilot_sections(s, now),
                VendorSnapshot::Zai(s) => zai_sections(s, now, pace_tolerance),
                VendorSnapshot::Openrouter(s) => openrouter_sections(s, prefs),
                VendorSnapshot::Deepseek(s) => deepseek_sections(s, prefs),
                VendorSnapshot::Kimi(s) => kimi_sections(s, now, pace_tolerance),
                VendorSnapshot::Kilo(s) => kilo_sections(s, prefs),
                VendorSnapshot::Novita(s) => novita_sections(s, prefs),
                VendorSnapshot::Moonshot(s) => moonshot_sections(s, prefs),
                VendorSnapshot::Grok(s) => grok_sections(s, prefs),
                VendorSnapshot::SuperGrok(s) => supergrok_sections(s, now),
                VendorSnapshot::Grokbot(s) => grokbot_sections(s, now),
                VendorSnapshot::Antigravity(s) => antigravity_sections(s, now),
                VendorSnapshot::Cursor(s) => cursor_sections(s, now),
                VendorSnapshot::Minimax(s) => minimax_sections(s, now, pace_tolerance),
                VendorSnapshot::Kiro(s) => kiro_sections(s, now),
                VendorSnapshot::NousResearch(s) => nous_sections(s, now),
                VendorSnapshot::OpenCodeGo(s) => opencode_go_sections(s, now, pace_tolerance),
                VendorSnapshot::CommandCode(s) => commandcode_sections(s, now),
                VendorSnapshot::Ollama(s) => ollama_sections(s, now, pace_tolerance),
                VendorSnapshot::Custom(s) => custom_sections(s),
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

fn openrouter_sections(
    s: &crate::usage::OpenRouterSnapshot,
    prefs: DisplayPrefs,
) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: s.label.clone(),
        right: None,
    }]);
    let pct = s.consumed_pct().clamp(0, 100) as u16;
    // OpenRouter states its own denominator — credits purchased — so there is
    // no user tank to fall back to and `None` is passed literally rather than
    // `prefs.display_limit`. A free-tier-only account buys nothing, so
    // `total_credits` is 0, `denominator` is `None`, and the money stays on the
    // bar. Routing `prefs.display_limit` in here instead would let a tank take
    // over in exactly that case and name the headline `percent` — while `pct`
    // below still came from `consumed_pct()`, which is 0 without credits. The
    // bar would read "0%" for an account with money in it.
    let denominator = balance::denominator(Some(s.total_credits), None);
    v.push(Section::Spacer);
    v.push_metric_with_headline(
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
        balance::resolve_headline(prefs.headline, denominator),
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
    // Figures read from the Cloud Code API fallback can lag what the local
    // product would show; say where they came from.
    if s.source == crate::usage::AntigravitySource::Remote {
        v.push(Section::Spacer);
        v.push(Section::Text {
            label: "Source".into(),
            value: "Google API".into(),
        });
    }
    v
}

/// A Cursor pool row. The billing cycle carries an exact window only when the
/// API stated both ends; when it did not, the row goes out with its reset time
/// and no window rather than a guessed month a frontend would pace as exact.
fn push_cursor_pool(v: &mut SectionBuilder, section: Section, s: &crate::usage::CursorSnapshot) {
    match s.cycle_window() {
        Some(window) => v.push_metric_in_window(section, s.reset_at, window),
        None => v.push_metric(section, s.reset_at),
    }
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
        push_cursor_pool(
            &mut v,
            Section::Metric {
                label: "Cursor Models".into(),
                pct: s.auto_pct.clamp(0, 100) as u16,
                severity: severity_for(s.auto_pct),
                value_label: format!("{}%", s.auto_pct),
                footnote: "Auto + Composer".into(),
            },
            s,
        );
        v.push(Section::Spacer);
        push_cursor_pool(
            &mut v,
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
            s,
        );
        if let Some(used) = s.on_demand_used_cents {
            v.push(Section::Spacer);
            v.push(Section::Text {
                label: "On-Demand".into(),
                value: match s.on_demand_limit_cents {
                    Some(limit) => format!(
                        "{} / {}",
                        crate::usage::fmt_minor(used, 2, Some("USD")),
                        crate::usage::fmt_minor(limit, 2, Some("USD"))
                    ),
                    None => crate::usage::fmt_minor(used, 2, Some("USD")),
                },
            });
        }
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
        ("Monthly", s.monthly_window().as_ref()),
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
        sections.push(Section::Text {
            label: "Credits".into(),
            value: usd(credits.remaining()),
        });
    }
    sections
}

fn opencode_go_sections(
    s: &crate::opencode_go::types::Usage,
    now: DateTime<Utc>,
    tol: u32,
) -> SectionBuilder {
    use crate::opencode_go::vendor::{ROLLING_WINDOW, WEEKLY_WINDOW};

    let mut sections = SectionBuilder::new(vec![Section::Title {
        left: "OpenCode Go".into(),
        right: None,
    }]);
    let mut any = false;
    for (label, window, duration) in [
        ("Rolling (5h)", s.rolling.as_ref(), ROLLING_WINDOW),
        ("Weekly (7d)", s.weekly.as_ref(), WEEKLY_WINDOW),
    ] {
        let Some(window) = window else {
            continue;
        };
        any = true;
        let pct = window.percent.round().clamp(0.0, 100.0) as i32;
        let projected = crate::usage::UsageWindow {
            utilization_pct: pct,
            resets_at: Some(window.resets_at),
            window_duration: duration,
        };
        push_window(&mut sections, label, &projected, now, tol, true);
    }
    // Monthly keeps its reset countdown but no pacing and no `window_secs`:
    // the cycle follows the subscription date (28/29/31-day months), so no
    // fixed denominator is exact. `push_metric` (not `push_metric_in_window`)
    // is what withholds the window from machine-readable frontends.
    if let Some(window) = s.monthly.as_ref() {
        any = true;
        let pct = window.percent.round().clamp(0.0, 100.0) as i32;
        sections.push_metric(
            Section::Metric {
                label: "Monthly".into(),
                pct: pct as u16,
                severity: severity_for(pct),
                value_label: format!("{pct}%"),
                footnote: format!(
                    "Resets in {}",
                    countdown::format(Some(window.resets_at), now)
                ),
            },
            Some(window.resets_at),
        );
    }
    if !any {
        sections.push(Section::Spacer);
        sections.push(Section::Text {
            label: "".into(),
            value: "  no usage windows reported".into(),
        });
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

fn kilo_sections(s: &crate::usage::KiloSnapshot, prefs: DisplayPrefs) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![
        Section::Title {
            left: s.label.clone(),
            right: None,
        },
        Section::Spacer,
    ]);
    push_balance_headline(
        &mut v,
        "Balance",
        s.balance,
        "USD",
        crate::kilo::vendor::severity(s),
        None,
        prefs,
    );
    v
}

fn novita_sections(s: &crate::usage::NovitaSnapshot, prefs: DisplayPrefs) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![
        Section::Title {
            left: "Novita".into(),
            right: None,
        },
        Section::Spacer,
    ]);
    // `credit_limit` is a credit line Novita extends, not a cap on spend, so it
    // is not a denominator — it stays in the breakdown.
    push_balance_headline(
        &mut v,
        "Balance",
        s.available,
        "USD",
        crate::novita::vendor::severity(s),
        None,
        prefs,
    );
    v.push(Section::Block {
        label: "Breakdown".into(),
        body: vec![format!(
            "top-up ${:.2} · credit limit ${:.2}",
            s.cash, s.credit_limit
        )],
    });
    if s.outstanding > 0.0 {
        v.push(Section::Spacer);
        v.push(Section::Block {
            label: "Owed".into(),
            body: vec![usd(s.outstanding)],
        });
    }
    v
}

fn moonshot_sections(s: &crate::usage::MoonshotSnapshot, prefs: DisplayPrefs) -> SectionBuilder {
    let cur = &s.currency;
    let fmt = |v: f64| money(v, cur);
    let mut v = SectionBuilder::new(vec![
        Section::Title {
            left: "Kimi (Moonshot)".into(),
            right: None,
        },
        Section::Spacer,
    ]);
    push_balance_headline(
        &mut v,
        "Balance",
        s.available,
        cur,
        crate::moonshot::vendor::severity(s),
        None,
        prefs,
    );
    v.push(Section::Block {
        label: "Breakdown".into(),
        body: vec![format!("cash {} · voucher {}", fmt(s.cash), fmt(s.voucher))],
    });
    v
}

fn grok_sections(s: &crate::usage::GrokSnapshot, prefs: DisplayPrefs) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![
        Section::Title {
            left: "Grok (xAI)".into(),
            right: None,
        },
        Section::Spacer,
    ]);
    push_balance_headline(
        &mut v,
        "Prepaid balance",
        s.balance,
        "USD",
        crate::grok::vendor::severity(s),
        None,
        prefs,
    );
    v
}

/// A user-declared `[[custom]]` provider: the plan (if the response carried
/// one), one gauge per configured metric, then the free-form text rows. The
/// vendor never states a reset countdown of its own; a metric's `resets_at`
/// and optional exact `window_secs` ride along as reset metadata so every
/// frontend paces it exactly like a built-in.
fn custom_sections(s: &crate::custom::types::CustomSnapshot) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![
        Section::Title {
            left: s.plan.clone().unwrap_or_default(),
            right: None,
        },
        Section::Spacer,
    ]);
    for metric in &s.metrics {
        let pct = metric.pct.min(100);
        let section = Section::Metric {
            label: metric.label.clone(),
            pct,
            severity: severity_for(i32::from(pct)),
            value_label: format!("{pct}%"),
            footnote: metric.footnote.clone(),
        };
        match metric.window_secs {
            Some(secs) => v.push_metric_in_window(
                section,
                metric.resets_at,
                chrono::Duration::seconds(i64::try_from(secs).unwrap_or(i64::MAX)),
            ),
            None => v.push_metric(section, metric.resets_at),
        }
    }
    if !s.texts.is_empty() {
        v.push(Section::Spacer);
        for text in &s.texts {
            v.push(Section::Text {
                label: text.label.clone(),
                value: text.value.clone(),
            });
        }
    }
    v
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
    let metric = Section::Metric {
        label: format!("{} usage", s.period.label()),
        pct: pct.clamp(0, 100) as u16,
        severity: severity_for(pct),
        value_label: format!("{pct}%"),
        footnote: String::new(),
    };
    // Only the weekly period has an exact length; a month varies and an
    // unknown period says nothing, so neither can be paced.
    if s.period == crate::usage::SuperGrokPeriod::Weekly {
        v.push_metric_in_window(metric, s.reset_at, chrono::Duration::days(7));
    } else {
        v.push_metric(metric, s.reset_at);
    }
    for product in &s.products {
        // Product slices share the overall pool. They must not carry the
        // window reset or a severity colour — only the overall usage meter
        // is the binding constraint. The "Breakdown" group lets frontends
        // draw them compactly under a heading instead of as peers of it.
        v.push_metric_in_group(
            Section::Metric {
                label: product.label.clone(),
                pct: product.percent.clamp(0, 100) as u16,
                severity: PaceSeverity::Low,
                value_label: format!("{}%", product.percent),
                footnote: String::new(),
            },
            "Breakdown",
        );
    }
    // A $0.00 prepaid row reads as "you have no money" when the field merely
    // says no credit was purchased on top of the subscription — and a unified
    // billing account keeps its real dollars in the Management API wallet
    // (`[grok]`), not here. Show the row only when there is credit to show.
    if let Some(bal) = s.prepaid_balance.filter(|bal| *bal > 0.0) {
        v.push(Section::Spacer);
        v.push(Section::Text {
            label: "Prepaid API".into(),
            value: usd(bal),
        });
    }
    push_reset_credits(&mut v, &s.reset_credits, now);
    v
}

fn ollama_sections(
    s: &crate::usage::OllamaSnapshot,
    now: DateTime<Utc>,
    pace_tolerance: u32,
) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: format!("Ollama Cloud {}", s.plan),
        right: None,
    }]);
    if let Some(w) = &s.session {
        push_window(&mut v, "Session (5h)", w, now, pace_tolerance, true);
    }
    if let Some(w) = &s.weekly {
        push_window(&mut v, "Weekly", w, now, pace_tolerance, true);
    }
    if let Some(w) = &s.monthly {
        push_window(&mut v, "Monthly", w, now, pace_tolerance, true);
    }
    push_top_models(&mut v, &s.session_models, "Top models (5h)");
    push_top_models(&mut v, &s.weekly_models, "Top models (weekly)");
    push_top_models(&mut v, &s.monthly_models, "Top models (monthly)");
    if let Some(cost) = &s.activity_cost {
        v.push(Section::Spacer);
        v.push(Section::Block {
            label: "Activity".into(),
            body: vec![format!(
                "{} · {}",
                usd_str(cost),
                s.activity_period.as_deref().unwrap_or("last 4 weeks")
            )],
        });
    }
    v
}

fn push_top_models(
    sections: &mut SectionBuilder,
    models: &[crate::usage::OllamaModelUsage],
    label: &str,
) {
    if models.is_empty() {
        return;
    }
    let mut sorted: Vec<&crate::usage::OllamaModelUsage> = models.iter().collect();
    sorted.sort_by_key(|m| std::cmp::Reverse(m.request_count));
    let body: Vec<String> = sorted
        .into_iter()
        .take(5)
        .map(|m| format!("{}: {} requests", m.name, m.request_count))
        .collect();
    sections.push(Section::Spacer);
    sections.push(Section::Block {
        label: label.into(),
        body,
    });
}

fn usd_str(cost: &str) -> String {
    cost.parse::<f64>()
        .map(usd)
        .unwrap_or_else(|_| cost.to_string())
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

fn deepseek_sections(s: &crate::usage::DeepseekSnapshot, prefs: DisplayPrefs) -> SectionBuilder {
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
    // `/user/balance` reports money remaining and no denominator at all, so
    // `api_limit` is unconditionally `None` here.
    push_balance_headline(
        &mut v,
        "Balance",
        s.balance,
        currency,
        crate::deepseek::vendor::severity(s),
        None,
        prefs,
    );
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
    if s.has_weekly {
        let w = window(s.weekly_pct(), s.weekly_reset_at, WEEKLY_WINDOW);
        push_window(&mut v, "Weekly quota", &w, now, tol, false);
    }
    if let Some(monthly_pct) = s.monthly_pct {
        // The monthly pool resets from the order date, so there is no fixed
        // window length: the metric carries its reset but no window metadata,
        // and nothing paces it.
        v.push(Section::Spacer);
        v.push_metric(
            Section::Metric {
                label: "Monthly".into(),
                pct: monthly_pct.clamp(0, 100) as u16,
                severity: severity_for(monthly_pct),
                value_label: format!("{monthly_pct}%"),
                footnote: format!("Resets in {}", countdown::format(s.monthly_reset_at, now)),
            },
            s.monthly_reset_at,
        );
    }

    v
}

/// Grok Bot: one weekly meter for the included pool — plus the honest
/// window length when both period instants were reported, so the report
/// carries exact `window_secs` — or the no-included-allowance state, which is
/// a text row, never a 0% meter.
fn grokbot_sections(s: &crate::usage::GrokbotSnapshot, now: DateTime<Utc>) -> SectionBuilder {
    let mut v = SectionBuilder::new(vec![Section::Title {
        left: s.plan.clone(),
        right: None,
    }]);
    v.push(Section::Spacer);
    if !s.has_included_allowance {
        v.push(Section::Text {
            label: "Included usage".into(),
            value: "no included allowance on this account".into(),
        });
        return v;
    }
    let metric = Section::Metric {
        label: "Weekly".into(),
        pct: s.weekly_pct.clamp(0, 100) as u16,
        severity: severity_for(s.weekly_pct),
        value_label: format!("{}%", s.weekly_pct),
        footnote: format!("Resets in {}", countdown::format(s.reset_at, now)),
    };
    match s.window {
        Some(window) => v.push_metric_in_window(metric, s.reset_at, window),
        None => v.push_metric(metric, s.reset_at),
    }
    // At 100% with the account still serving, on-demand may be picking up the
    // rest — a footnote row, not its own meter.
    if let Some(note) = s.on_demand_note() {
        v.push(Section::Spacer);
        v.push(Section::Text {
            label: "On-demand".into(),
            value: note.into(),
        });
    }
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
    sections.push_metric_in_window(
        Section::Metric {
            label: label.into(),
            pct,
            severity: severity_for(pct as i32),
            value_label: format!("{pct}%"),
            footnote,
        },
        w.resets_at,
        w.window_duration,
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
    let value_w = crate::display::text_width(value_label) as u16 + 2;
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
        ready_with(snapshot, DisplayPrefs::default())
    }

    fn ready_with(snapshot: VendorSnapshot, display: DisplayPrefs) -> TabState {
        TabState::Ready(Box::new(crate::tui::app::ReadyTab {
            snapshot,
            stale: false,
            last_error: None,
            fetched_at: Some(now() - chrono::Duration::seconds(15)),
            display,
        }))
    }

    fn supergrok(period: crate::usage::SuperGrokPeriod) -> VendorSnapshot {
        VendorSnapshot::SuperGrok(crate::usage::SuperGrokSnapshot {
            plan: "SuperGrok".into(),
            account: "digest".into(),
            weekly_pct: 40,
            period,
            reset_at: Some(now() + chrono::Duration::days(2)),
            prepaid_balance: None,
            reset_credits: crate::usage::ResetCredits::default(),
            products: Vec::new(),
        })
    }

    fn only_metric(sections: &[SectionProjection]) -> &SectionProjection {
        let mut metrics = sections
            .iter()
            .filter(|projection| matches!(projection.section, Section::Metric { .. }));
        let metric = metrics.next().expect("one metric row");
        assert!(metrics.next().is_none(), "expected exactly one metric row");
        metric
    }

    /// Only a rolling window has a length a frontend can pace against. The
    /// shared `UsageWindow` helper always knows it; SuperGrok knows it for a
    /// week and not for a month, whose length varies.
    #[test]
    fn window_length_is_reported_only_when_exact() {
        use crate::usage::SuperGrokPeriod;

        let weekly =
            sections_with_metadata_for(&ready(supergrok(SuperGrokPeriod::Weekly)), now(), 5);
        assert_eq!(only_metric(&weekly).window, Some(chrono::Duration::days(7)));

        let monthly =
            sections_with_metadata_for(&ready(supergrok(SuperGrokPeriod::Monthly)), now(), 5);
        assert_eq!(only_metric(&monthly).window, None);

        let unknown =
            sections_with_metadata_for(&ready(supergrok(SuperGrokPeriod::Unknown)), now(), 5);
        assert_eq!(only_metric(&unknown).window, None);

        let kimi = sections_with_metadata_for(
            &ready(VendorSnapshot::Kimi(KimiSnapshot {
                plan: None,
                weekly_limit: 100,
                weekly_used: 10,
                weekly_remaining: 90,
                weekly_reset_at: Some(now() + chrono::Duration::days(3)),
                has_weekly: true,
                monthly_pct: None,
                monthly_reset_at: None,
                window_limit: 0,
                window_used: 0,
                window_remaining: 0,
                window_reset_at: None,
            })),
            now(),
            5,
        );
        assert_eq!(
            only_metric(&kimi).window,
            Some(crate::kimi::vendor::WEEKLY_WINDOW)
        );
    }

    #[test]
    fn cursor_pools_carry_the_billing_cycle_window_only_when_it_is_exact() {
        let mut snap = cursor_snap();
        snap.cycle_start = Some(now() - chrono::Duration::days(22));
        let exact =
            sections_with_metadata_for(&ready(VendorSnapshot::Cursor(snap.clone())), now(), 5);
        let windows: Vec<_> = exact
            .iter()
            .filter(|p| matches!(p.section, Section::Metric { .. }))
            .map(|p| p.window)
            .collect();
        assert_eq!(
            windows,
            vec![
                Some(chrono::Duration::days(31)),
                Some(chrono::Duration::days(31))
            ]
        );

        // Without `billingCycleStart` the cycle length is unknown. Reporting a
        // guessed month here would reach a frontend as an exact window and be
        // paced as one; every pool goes out with no window instead. The reset
        // time is unaffected.
        snap.cycle_start = None;
        let unknown = sections_with_metadata_for(&ready(VendorSnapshot::Cursor(snap)), now(), 5);
        let pools: Vec<_> = unknown
            .iter()
            .filter(|p| matches!(p.section, Section::Metric { .. }))
            .collect();
        assert_eq!(pools.len(), 2);
        assert!(
            pools.iter().all(|p| p.window.is_none()),
            "an unstated billing cycle must not report a window length"
        );
        assert!(
            pools.iter().all(|p| p.reset_at.is_some()),
            "the reset time still travels with the row"
        );
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
        let sections = sections_for(&TabState::error("token expired"), now(), 5);
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
    fn error_state_keeps_oauth_plan_as_title() {
        let sections = sections_for(
            &TabState::error_with_plan("HTTP 401", Some("Claude Max 5x".into())),
            now(),
            5,
        );
        assert!(matches!(
            &sections[0],
            Section::Title { left, .. } if left == "Claude Max 5x"
        ));
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { value, .. } if value.contains("HTTP 401")
        )));
        assert!(!sections.iter().any(|s| matches!(s, Section::Metric { .. })));
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
            products: Vec::new(),
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
            products: Vec::new(),
        };
        let sections = sections_for(&ready(VendorSnapshot::SuperGrok(snap)), now, 5);
        assert!(!sections.iter().any(|section| matches!(
            section,
            Section::Text { label, .. } if label == "Resets"
        )));
        // Zero prepaid is noise (see `supergrok_hides_a_zero_prepaid_balance`),
        // so even a present-but-zero field draws no row.
        assert!(!sections.iter().any(|section| matches!(
            section,
            Section::Text { label, .. } if label == "Prepaid API"
        )));
    }

    /// A $0.00 prepaid line reads as "no money" when the billing document
    /// merely reports that nothing was purchased on top of the subscription.
    /// The row appears only when there is credit to show.
    #[test]
    fn supergrok_hides_a_zero_prepaid_balance_but_keeps_a_real_one() {
        let now = now();
        let base = |prepaid: Option<f64>| crate::usage::SuperGrokSnapshot {
            plan: "SuperGrok".into(),
            account: "scope".into(),
            weekly_pct: 40,
            period: crate::usage::SuperGrokPeriod::Weekly,
            reset_at: Some(now + chrono::Duration::days(6)),
            prepaid_balance: prepaid,
            reset_credits: ResetCredits::default(),
            products: Vec::new(),
        };
        let labels = |snap| {
            sections_for(&ready(VendorSnapshot::SuperGrok(snap)), now, 5)
                .into_iter()
                .filter_map(|section| match section {
                    Section::Text { label, value, .. } => Some((label, value)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert!(
            !labels(base(Some(0.0)))
                .iter()
                .any(|(label, _)| label == "Prepaid API")
        );
        assert_eq!(
            labels(base(Some(4.22))).last(),
            Some(&("Prepaid API".to_string(), "$4.22".to_string()))
        );
    }

    #[test]
    fn supergrok_lists_product_slices_beside_the_overall_meter() {
        let now = now();
        let snap = crate::usage::SuperGrokSnapshot {
            plan: "SuperGrok".into(),
            account: "scope".into(),
            weekly_pct: 90,
            period: crate::usage::SuperGrokPeriod::Weekly,
            reset_at: Some(now + chrono::Duration::days(3)),
            prepaid_balance: None,
            reset_credits: ResetCredits::default(),
            products: vec![
                crate::usage::SuperGrokProduct {
                    label: "Grok Build".into(),
                    percent: 87,
                },
                crate::usage::SuperGrokProduct {
                    label: "Grok Chat".into(),
                    percent: 3,
                },
            ],
        };
        let labels: Vec<_> = sections_for(&ready(VendorSnapshot::SuperGrok(snap)), now, 5)
            .into_iter()
            .filter_map(|section| match section {
                Section::Metric { label, pct, .. } => Some((label, pct)),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels,
            vec![
                ("Weekly usage".into(), 90),
                ("Grok Build".into(), 87),
                ("Grok Chat".into(), 3),
            ]
        );
    }

    /// Product slices report the "Breakdown" group so frontends can draw them
    /// under a heading; the overall meter stays ungrouped, and neither gains
    /// reset metadata it must not have.
    #[test]
    fn supergrok_product_slices_carry_the_breakdown_group() {
        let now = now();
        let snap = crate::usage::SuperGrokSnapshot {
            plan: "SuperGrok".into(),
            account: "scope".into(),
            weekly_pct: 90,
            period: crate::usage::SuperGrokPeriod::Weekly,
            reset_at: Some(now + chrono::Duration::days(3)),
            prepaid_balance: None,
            reset_credits: ResetCredits::default(),
            products: vec![crate::usage::SuperGrokProduct {
                label: "Grok Build".into(),
                percent: 87,
            }],
        };
        let projected = sections_with_metadata_for(&ready(VendorSnapshot::SuperGrok(snap)), now, 5);
        let mut metrics = projected
            .iter()
            .filter(|p| matches!(p.section, Section::Metric { .. }));
        let overall = metrics.next().expect("overall usage metric");
        let build = metrics.next().expect("one product slice metric");
        assert!(metrics.next().is_none(), "expected exactly two metric rows");
        assert_eq!(overall.group, None);
        assert!(overall.reset_at.is_some());
        assert_eq!(build.group, Some("Breakdown"));
        assert_eq!(build.reset_at, None);
        assert_eq!(build.window, None);
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
            has_weekly: true,
            monthly_pct: None,
            monthly_reset_at: None,
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
            has_weekly: true,
            monthly_pct: None,
            monthly_reset_at: None,
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
            has_weekly: true,
            monthly_pct: None,
            monthly_reset_at: None,
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

    /// The newer `usages`-map shape has no weekly bucket: the panel drops the
    /// weekly row and lists the monthly pool — with its reset, but with no
    /// window metadata, because the reset hangs on the order date and there
    /// is no fixed length to pace against.
    #[test]
    fn kimi_sections_on_the_monthly_shape_drop_weekly_and_add_monthly() {
        let now = now();
        let snap = KimiSnapshot {
            plan: Some("Allegretto".into()),
            weekly_limit: 0,
            weekly_used: 0,
            weekly_remaining: 0,
            weekly_reset_at: None,
            has_weekly: false,
            monthly_pct: Some(42),
            monthly_reset_at: Some(now + chrono::Duration::days(30)),
            window_limit: 100,
            window_used: 15,
            window_remaining: 85,
            window_reset_at: Some(now + chrono::Duration::hours(2)),
        };
        let projected =
            sections_with_metadata_for(&ready(VendorSnapshot::Kimi(snap.clone())), now, 5);
        let metrics: Vec<_> = projected
            .iter()
            .filter(|p| matches!(p.section, Section::Metric { .. }))
            .collect();
        assert_eq!(metrics.len(), 2);
        let monthly = metrics
            .iter()
            .find(|p| matches!(&p.section, Section::Metric { label, .. } if label == "Monthly"))
            .expect("a Monthly metric");
        assert!(matches!(
            &monthly.section,
            Section::Metric { value_label, footnote, .. }
            if value_label == "42%" && footnote == "Resets in 30d 0h"
        ));
        assert_eq!(monthly.reset_at, Some(now + chrono::Duration::days(30)));
        assert_eq!(monthly.window, None, "no fixed window length, no pacing");
        assert!(!metrics.iter().any(
            |p| matches!(&p.section, Section::Metric { label, .. } if label == "Weekly quota")
        ));

        // The Overview cells follow the same presence rules.
        let (_, cells) = compact_cells(&VendorSnapshot::Kimi(snap));
        let texts: Vec<&str> = cells.iter().map(|(text, _)| text.as_str()).collect();
        assert_eq!(texts, ["5h 15%", "mo 42%"]);
    }

    fn cursor_snap() -> crate::usage::CursorSnapshot {
        crate::usage::CursorSnapshot {
            plan: "Ultra".into(),
            auto_pct: 98,
            api_pct: 100,
            total_pct: 99,
            unlimited: false,
            on_demand_enabled: false,
            on_demand_used_cents: None,
            on_demand_limit_cents: None,
            reset_at: Some(now() + chrono::Duration::days(9)),
            cycle_start: None,
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
        let error = TabState::error("bad\x1b]52;c;Y2FuYXJ5\x07 value");
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
        let mut snapshot = cursor_snap();
        snapshot.on_demand_enabled = true;
        snapshot.on_demand_used_cents = Some(1785);
        snapshot.on_demand_limit_cents = Some(35000);
        let sections = sections_for(&ready(VendorSnapshot::Cursor(snapshot)), now(), 5);
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
        assert!(sections.iter().any(|section| matches!(
            section,
            Section::Text { label, value }
                if label == "On-Demand" && value == "$17.85 / $350.00"
        )));
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

    fn grokbot_snap() -> crate::usage::GrokbotSnapshot {
        crate::usage::GrokbotSnapshot {
            plan: "Grok Bot Plan".into(),
            has_included_allowance: true,
            weekly_pct: 42,
            has_available_usage: true,
            on_demand_enabled: false,
            period_start: Some(now() - chrono::Duration::days(3)),
            reset_at: Some(now() + chrono::Duration::days(4)),
            window: Some(chrono::Duration::days(7)),
        }
    }

    #[test]
    fn grokbot_sections_show_one_weekly_meter_with_the_derived_window() {
        let sections =
            sections_with_metadata_for(&ready(VendorSnapshot::Grokbot(grokbot_snap())), now(), 5);
        let metric = only_metric(&sections);
        let Section::Metric {
            label,
            value_label,
            footnote,
            ..
        } = &metric.section
        else {
            unreachable!()
        };
        assert_eq!(label, "Weekly");
        assert_eq!(value_label, "42%");
        assert!(footnote.contains("Resets in"), "{footnote}");
        // The honest derived window, so a frontend paces against 7d exactly.
        assert_eq!(metric.window, Some(chrono::Duration::days(7)));
        assert_eq!(metric.reset_at, grokbot_snap().reset_at);
    }

    #[test]
    fn grokbot_no_allowance_state_is_a_text_row_not_a_meter() {
        let snap = crate::usage::GrokbotSnapshot {
            has_included_allowance: false,
            weekly_pct: 0,
            ..grokbot_snap()
        };
        let sections = sections_for(&ready(VendorSnapshot::Grokbot(snap.clone())), now(), 5);
        assert!(
            sections
                .iter()
                .all(|s| !matches!(s, Section::Metric { .. })),
            "no meter without an included allowance"
        );
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { value, .. } if value.contains("no included allowance")
        )));
        assert_eq!(headline_pct(&VendorSnapshot::Grokbot(snap)), None);
        let (_, cells) = compact_cells(&VendorSnapshot::Grokbot(grokbot_snap()));
        assert_eq!(cells.len(), 1);
        assert!(cells[0].0.contains("42%"), "{cells:?}");
    }

    #[test]
    fn grokbot_on_demand_footnote_is_a_text_row_when_it_applies() {
        let mut snap = grokbot_snap();
        snap.weekly_pct = 100;
        snap.has_available_usage = true;
        snap.on_demand_enabled = true;
        let sections = sections_for(&ready(VendorSnapshot::Grokbot(snap)), now(), 5);
        assert!(sections.iter().any(|s| matches!(
            s,
            Section::Text { label, value } if label == "On-demand" && value.contains("on-demand")
        )));

        let mut snap = grokbot_snap();
        snap.weekly_pct = 100;
        snap.has_available_usage = true;
        snap.on_demand_enabled = false;
        let sections = sections_for(&ready(VendorSnapshot::Grokbot(snap)), now(), 5);
        assert!(
            sections
                .iter()
                .all(|s| !matches!(s, Section::Text { label, .. } if label == "On-demand")),
            "on-demand off: no footnote"
        );
    }

    #[test]
    fn grokbot_without_a_period_start_reports_no_window() {
        let mut snap = grokbot_snap();
        snap.period_start = None;
        snap.window = None;
        let sections = sections_with_metadata_for(&ready(VendorSnapshot::Grokbot(snap)), now(), 5);
        assert_eq!(only_metric(&sections).window, None);
    }

    #[test]
    fn schema_drift_and_generic_code_zero_diagnostics_are_visible_without_http_labels() {
        let snap = KimiSnapshot {
            plan: None,
            weekly_limit: 100,
            weekly_used: 10,
            weekly_remaining: 90,
            weekly_reset_at: None,
            has_weekly: true,
            monthly_pct: None,
            monthly_reset_at: None,
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
                has_weekly: true,
                monthly_pct: None,
                monthly_reset_at: None,
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

    fn antigravity_snap(source: crate::usage::AntigravitySource) -> VendorSnapshot {
        VendorSnapshot::Antigravity(crate::usage::AntigravitySnapshot {
            plan: "Pro".into(),
            account: "acct:test".into(),
            source,
            session: Some(UsageWindow {
                utilization_pct: 43,
                resets_at: Some(now() + chrono::Duration::hours(2)),
                window_duration: chrono::Duration::hours(5),
            }),
            weekly: None,
            third_party_session: None,
            third_party_weekly: None,
        })
    }

    /// Figures read off the API while nothing runs say so; a running product's
    /// do not, since that is the normal case.
    #[test]
    fn antigravity_names_the_remote_source_and_only_that() {
        use crate::usage::AntigravitySource;

        let remote = sections_for(
            &ready(antigravity_snap(AntigravitySource::Remote)),
            now(),
            5,
        );
        let n = remote.len();
        assert!(matches!(remote[n - 2], Section::Spacer));
        assert!(matches!(
            &remote[n - 1],
            Section::Text { label, value }
                if label == "Source" && value == "Google API"
        ));

        let local = sections_for(&ready(antigravity_snap(AntigravitySource::Local)), now(), 5);
        assert!(
            !local
                .iter()
                .any(|s| matches!(s, Section::Text { label, .. } if label == "Source"))
        );
    }

    /// A custom provider's rows come out in declaration order: title, gauges,
    /// then texts. Only a metric that states its window length carries one;
    /// every metric's own `resets_at` rides along as reset metadata.
    #[test]
    fn custom_sections_follow_declaration_order_and_carry_reset_metadata() {
        use crate::custom::types::{CustomMetric, CustomSnapshot, CustomText};

        let session_reset = now() + chrono::Duration::hours(3);
        let monthly_reset = now() + chrono::Duration::days(12);
        let snapshot = VendorSnapshot::Custom(CustomSnapshot {
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
                    pct: 120,
                    footnote: String::new(),
                    resets_at: Some(monthly_reset),
                    window_secs: None,
                },
            ],
            texts: vec![CustomText {
                label: "Region".into(),
                value: "eu".into(),
            }],
        });

        let sections = sections_with_metadata_for(&ready(snapshot.clone()), now(), 5);
        assert!(matches!(
            &sections[0].section,
            Section::Title { left, right } if left == "Team" && right.is_some()
        ));
        assert!(matches!(sections[1].section, Section::Spacer));
        assert!(matches!(
            &sections[2].section,
            Section::Metric { label, pct, value_label, footnote, .. }
                if label == "Session" && *pct == 40 && value_label == "40%" && footnote == "40 of 100"
        ));
        assert_eq!(sections[2].reset_at, Some(session_reset));
        assert_eq!(sections[2].window, Some(chrono::Duration::hours(5)));
        // An over-100 percentage is clamped for the gauge; no window is invented.
        assert!(matches!(
            &sections[3].section,
            Section::Metric { label, pct, value_label, .. }
                if label == "Monthly" && *pct == 100 && value_label == "100%"
        ));
        assert_eq!(sections[3].reset_at, Some(monthly_reset));
        assert_eq!(sections[3].window, None);
        assert!(matches!(sections[4].section, Section::Spacer));
        assert!(matches!(
            &sections[5].section,
            Section::Text { label, value } if label == "Region" && value == "eu"
        ));
        assert_eq!(sections.len(), 6);

        let (plan, cells) = compact_cells(&snapshot);
        assert_eq!(plan, "Team");
        assert_eq!(cells[0].0, "Session 40%");
        assert_eq!(cells[1].0, "Monthly 120%");
        assert_eq!(headline_pct(&snapshot), Some(40));

        // No plan and no texts: an empty title, no trailing spacer, no bar.
        let bare = VendorSnapshot::Custom(CustomSnapshot {
            plan: None,
            metrics: vec![],
            texts: vec![],
        });
        let sections = sections_with_metadata_for(&ready(bare.clone()), now(), 5);
        assert!(matches!(&sections[0].section, Section::Title { left, .. } if left.is_empty()));
        assert_eq!(sections.len(), 2);
        assert_eq!(compact_cells(&bare), (String::new(), vec![]));
        assert_eq!(headline_pct(&bare), None);
    }

    // --- prepaid-balance tank + headline (display_limit) ---

    fn deepseek(balance: f64) -> VendorSnapshot {
        VendorSnapshot::Deepseek(crate::usage::DeepseekSnapshot {
            is_available: true,
            balance,
            granted: balance,
            topped_up: 0.0,
            currency: "USD".into(),
        })
    }

    fn balance_row(sections: &[SectionProjection]) -> &SectionProjection {
        sections
            .iter()
            .find(|projection| {
                matches!(&projection.section,
                    Section::Metric { label, .. } | Section::Text { label, .. }
                        if label == "Balance" || label == "Prepaid balance")
            })
            .expect("a balance row")
    }

    /// No `display_limit`: nothing supplies a denominator, so the row stays
    /// free text rather than becoming a fabricated 0% gauge.
    #[test]
    fn a_balance_vendor_without_a_tank_keeps_its_plain_text_row() {
        let sections = sections_with_metadata_for(&ready(deepseek(5.50)), now(), 5);
        assert!(matches!(
            &balance_row(&sections).section,
            Section::Text { value, .. } if value == "$5.50"
        ));
    }

    /// A tank turns the same row into a consumed meter. The default headline
    /// for a balance vendor is the money figure, and setting `display_limit`
    /// alone does not change that — the percentage rides in the detail.
    #[test]
    fn a_tank_meters_the_balance_without_moving_the_money_off_the_bar() {
        let prefs = DisplayPrefs::balance(Some(200.0), crate::balance::Headline::Amount);
        let sections = sections_with_metadata_for(&ready_with(deepseek(50.0), prefs), now(), 5);
        let row = balance_row(&sections);
        assert_eq!(row.headline, MetricHeadline::Value);
        match &row.section {
            Section::Metric {
                pct,
                value_label,
                footnote,
                ..
            } => {
                assert_eq!(*pct, 75);
                assert_eq!(value_label, "$50.00");
                assert_eq!(footnote, "75% of $200.00 used ($50.00 left)");
            }
            _ => panic!("expected a metric"),
        }
    }

    /// Choosing `percent` moves the percentage onto the bar and the dollar
    /// figure into the detail line — the two numbers swap, neither is lost.
    #[test]
    fn choosing_percent_swaps_which_number_is_the_headline() {
        let prefs = DisplayPrefs::balance(Some(200.0), crate::balance::Headline::Percent);
        let sections = sections_with_metadata_for(&ready_with(deepseek(50.0), prefs), now(), 5);
        let row = balance_row(&sections);
        assert_eq!(row.headline, MetricHeadline::Percent);
        match &row.section {
            Section::Metric {
                pct,
                value_label,
                footnote,
                ..
            } => {
                assert_eq!(*pct, 75);
                assert_eq!(value_label, "75%");
                assert_eq!(footnote, "$50.00 of $200.00 left (75% used)");
            }
            _ => panic!("expected a metric"),
        }
    }

    /// `percent` with nothing to divide by keeps the amount on the bar rather
    /// than inventing a denominator.
    #[test]
    fn percent_without_a_tank_keeps_the_amount_on_the_bar() {
        let prefs = DisplayPrefs::balance(None, crate::balance::Headline::Percent);
        let sections = sections_with_metadata_for(&ready_with(deepseek(5.50), prefs), now(), 5);
        assert!(matches!(
            &balance_row(&sections).section,
            Section::Text { value, .. } if value == "$5.50"
        ));
    }

    /// A balance above the cap is 0% used; the money figure is what says how
    /// far above it sits, and it stays in the detail line.
    #[test]
    fn a_balance_over_the_cap_reads_as_zero_percent_with_the_money_in_the_detail() {
        let prefs = DisplayPrefs::balance(Some(20.0), crate::balance::Headline::Percent);
        let sections = sections_with_metadata_for(&ready_with(deepseek(50.0), prefs), now(), 5);
        match &balance_row(&sections).section {
            Section::Metric {
                pct,
                value_label,
                footnote,
                severity,
                ..
            } => {
                assert_eq!(*pct, 0);
                assert_eq!(value_label, "0%");
                assert!(footnote.contains("$50.00"), "{footnote}");
                assert_eq!(*severity, PaceSeverity::Low);
            }
            _ => panic!("expected a metric"),
        }
    }

    /// The money tier stays the floor: a nearly-empty wallet is critical even
    /// when the tank it is measured against is tiny enough to look healthy.
    #[test]
    fn severity_is_the_worse_of_the_money_tier_and_the_percentage_tier() {
        // $0.50 left of a $1 tank: 50% used is only "mid", but under the $1
        // USD floor the wallet itself is critical.
        let prefs = DisplayPrefs::balance(Some(1.0), crate::balance::Headline::Percent);
        let sections = sections_with_metadata_for(&ready_with(deepseek(0.50), prefs), now(), 5);
        match &balance_row(&sections).section {
            Section::Metric { severity, .. } => assert_eq!(*severity, PaceSeverity::Critical),
            _ => panic!("expected a metric"),
        }

        // $30 left of a $200 tank: the money tier is comfortable, but 85% of
        // the tank is gone and the meter has to say so.
        let prefs = DisplayPrefs::balance(Some(200.0), crate::balance::Headline::Percent);
        let sections = sections_with_metadata_for(&ready_with(deepseek(30.0), prefs), now(), 5);
        match &balance_row(&sections).section {
            Section::Metric { pct, severity, .. } => {
                assert_eq!(*pct, 85);
                assert_eq!(*severity, PaceSeverity::High);
            }
            _ => panic!("expected a metric"),
        }
    }

    /// Every balance-only vendor gets the same shape from one helper, so a
    /// tank and a headline behave identically across all of them.
    #[test]
    fn every_balance_only_vendor_meters_the_same_way() {
        let snapshots: Vec<VendorSnapshot> = vec![
            deepseek(50.0),
            VendorSnapshot::Kilo(crate::usage::KiloSnapshot {
                label: "Kilo".into(),
                balance: 50.0,
            }),
            VendorSnapshot::Novita(crate::usage::NovitaSnapshot {
                available: 50.0,
                cash: 50.0,
                credit_limit: 0.0,
                outstanding: 0.0,
            }),
            VendorSnapshot::Moonshot(crate::usage::MoonshotSnapshot {
                available: 50.0,
                cash: 50.0,
                voucher: 0.0,
                currency: "USD".into(),
            }),
            VendorSnapshot::Grok(crate::usage::GrokSnapshot { balance: 50.0 }),
        ];
        for snapshot in snapshots {
            let bare = sections_with_metadata_for(&ready(snapshot.clone()), now(), 5);
            assert!(
                matches!(&balance_row(&bare).section, Section::Text { .. }),
                "{snapshot:?} should keep a text row without a tank"
            );

            let prefs = DisplayPrefs::balance(Some(200.0), crate::balance::Headline::Percent);
            let metered =
                sections_with_metadata_for(&ready_with(snapshot.clone(), prefs), now(), 5);
            let row = balance_row(&metered);
            assert_eq!(row.headline, MetricHeadline::Percent, "{snapshot:?}");
            match &row.section {
                Section::Metric {
                    pct, value_label, ..
                } => {
                    assert_eq!(*pct, 75, "{snapshot:?}");
                    assert_eq!(value_label, "75%", "{snapshot:?}");
                }
                _ => panic!("{snapshot:?} should meter with a tank"),
            }
        }
    }

    /// OpenRouter states its own denominator, so a `display_limit` on it never
    /// applies — the fallback order puts the API's number first.
    #[test]
    fn openrouters_own_credits_win_over_a_configured_display_limit() {
        let snapshot = VendorSnapshot::Openrouter(crate::usage::OpenRouterSnapshot {
            label: "OpenRouter".into(),
            total_credits: 100.0,
            total_usage: 25.0,
            usage_daily: 0.0,
            usage_weekly: 0.0,
            usage_monthly: 0.0,
            is_free_tier: false,
            limit: None,
            limit_remaining: None,
        });
        // A wildly different tank size changes nothing: 25 of 100 is 25%.
        // `[openrouter]` carries no `display_limit`, so this can only arrive
        // through a hand-built `DisplayPrefs` — and the call site drops it.
        let prefs = DisplayPrefs::balance(Some(10_000.0), crate::balance::Headline::Percent);
        let sections = sections_with_metadata_for(&ready_with(snapshot, prefs), now(), 5);
        let metric = only_metric(&sections);
        assert_eq!(metric.headline, MetricHeadline::Percent);
        match &metric.section {
            Section::Metric { pct, .. } => assert_eq!(*pct, 25),
            _ => unreachable!(),
        }
    }

    /// The default for OpenRouter is the percent it always has a denominator
    /// for; the dollar figure stays available in `value` for the detail line.
    #[test]
    fn openrouter_defaults_to_percent_and_keeps_the_money_in_the_value() {
        let snapshot = VendorSnapshot::Openrouter(crate::usage::OpenRouterSnapshot {
            label: "OpenRouter".into(),
            total_credits: 100.0,
            total_usage: 25.0,
            usage_daily: 0.0,
            usage_weekly: 0.0,
            usage_monthly: 0.0,
            is_free_tier: false,
            limit: None,
            limit_remaining: None,
        });
        let sections = sections_with_metadata_for(
            &ready_with(
                snapshot,
                crate::config::Config::default().display_prefs(crate::vendor::VendorId::Openrouter),
            ),
            now(),
            5,
        );
        let metric = only_metric(&sections);
        assert_eq!(metric.headline, MetricHeadline::Percent);
        match &metric.section {
            Section::Metric { value_label, .. } => assert_eq!(value_label, "$75.00"),
            _ => unreachable!(),
        }
    }

    /// A free-tier-only OpenRouter account bought no credits, so there is no
    /// denominator and the money stays on the bar even at the percent default.
    ///
    /// The `prefs` loop is the regression: `openrouter_sections` used to route
    /// `prefs.display_limit` into `balance::denominator`, and with credits at 0
    /// the API value was dropped as unusable, so a tank took over and named the
    /// headline `percent` — while `pct` still came from `consumed_pct()`, which
    /// is 0 without credits. The bar read "0%" for an account holding money. No
    /// `[openrouter] display_limit` exists any more, and the call site passes
    /// `None` literally, so neither half of that can come back.
    #[test]
    fn a_free_tier_openrouter_account_has_no_denominator_and_shows_the_money() {
        let snapshot = VendorSnapshot::Openrouter(crate::usage::OpenRouterSnapshot {
            label: "OpenRouter".into(),
            total_credits: 0.0,
            total_usage: 0.0,
            usage_daily: 0.0,
            usage_weekly: 0.0,
            usage_monthly: 0.0,
            is_free_tier: true,
            limit: None,
            limit_remaining: None,
        });
        for prefs in [
            DisplayPrefs::default(),
            DisplayPrefs::balance(Some(200.0), crate::balance::Headline::Percent),
            DisplayPrefs::balance(Some(200.0), crate::balance::Headline::Amount),
        ] {
            let sections =
                sections_with_metadata_for(&ready_with(snapshot.clone(), prefs), now(), 5);
            let metric = only_metric(&sections);
            assert_eq!(metric.headline, MetricHeadline::Value, "{prefs:?}");
            match &metric.section {
                Section::Metric { value_label, .. } => {
                    assert_eq!(value_label, "$0.00", "{prefs:?}")
                }
                _ => unreachable!(),
            }
        }
    }

    /// The same corner with money actually in the account: an overdrawn or
    /// zero-credit snapshot must never be metered against a tank, because the
    /// percentage on that row does not come from one.
    #[test]
    fn a_credit_less_openrouter_snapshot_is_never_metered_against_a_tank() {
        let snapshot = VendorSnapshot::Openrouter(crate::usage::OpenRouterSnapshot {
            label: "OpenRouter".into(),
            total_credits: 0.0,
            total_usage: 0.0,
            usage_daily: 0.0,
            usage_weekly: 0.0,
            usage_monthly: 0.0,
            is_free_tier: false,
            limit: Some(50.0),
            limit_remaining: Some(50.0),
        });
        let prefs = DisplayPrefs::balance(Some(200.0), crate::balance::Headline::Percent);
        let sections = sections_with_metadata_for(&ready_with(snapshot, prefs), now(), 5);
        let metric = only_metric(&sections);
        assert_eq!(metric.headline, MetricHeadline::Value);
        match &metric.section {
            // Not "0%": the row reports the money, which is what this snapshot
            // actually knows.
            Section::Metric {
                pct, value_label, ..
            } => {
                assert_eq!(*pct, 0);
                assert_eq!(value_label, "$0.00");
            }
            _ => unreachable!(),
        }
    }

    /// Nothing else moved: a quota vendor's metrics stay percentages.
    #[test]
    fn a_quota_vendors_metrics_stay_percent_headlines() {
        let sections = sections_with_metadata_for(
            &ready(supergrok(crate::usage::SuperGrokPeriod::Weekly)),
            now(),
            5,
        );
        for projection in &sections {
            if matches!(projection.section, Section::Metric { .. }) {
                assert_eq!(projection.headline, MetricHeadline::Percent);
            }
        }
    }
}
