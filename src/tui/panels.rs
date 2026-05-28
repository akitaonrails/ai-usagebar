//! Native ratatui panels.
//!
//! Each vendor projects its snapshot into a sequence of [`Section`]s — either
//! a metric (gauge + footnote) or a free-form text block. The renderer lays
//! them out vertically with consistent spacing so every panel has the same
//! visual rhythm regardless of vendor.
//!
//! Progress bars use [`ratatui::widgets::Gauge`] which scales to the
//! available width — so on a wide monitor you get long, readable bars
//! instead of the 20-char Pango ones the Waybar tooltip is stuck with.

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Gauge, Paragraph};

use crate::countdown;
use crate::format::local_time_hms;
use crate::pacing::{self, PaceSeverity};
use crate::pango::severity_for;
use crate::theme::Theme;
use crate::tui::app::TabState;
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

/// Build the section list for the currently-active vendor's snapshot.
pub fn sections_for(tab: &TabState, now: DateTime<Utc>, pace_tolerance: u32) -> Vec<Section> {
    match tab {
        TabState::Loading => vec![
            Section::Spacer,
            Section::Text {
                label: "".into(),
                value: "  Loading…".into(),
            },
        ],
        TabState::Error(e) => vec![
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
        ],
        TabState::Ready(r) => {
            let snapshot = &r.snapshot;
            let last_error = &r.last_error;
            let mut sections = match snapshot {
                VendorSnapshot::Anthropic(s) => anthropic_sections(s, now, pace_tolerance),
                VendorSnapshot::Openai(s) => openai_sections(s, now, pace_tolerance),
                VendorSnapshot::Zai(s) => zai_sections(s, now),
                VendorSnapshot::Openrouter(s) => openrouter_sections(s),
            };
            // Inject the (already-absolute) fetched-at instant into the title
            // row, right-aligned. Pre-snapshotted in app::refresh_one so it
            // doesn't drift between redraws.
            let updated = match r.fetched_at {
                Some(at) => format!("Updated {}", local_time_hms(at)),
                None => "Updated —".to_string(),
            };
            if let Some(Section::Title { right, .. }) = sections.first_mut() {
                *right = Some(updated);
            }
            // Error footer (when present) still lives in the body.
            if let Some((code, msg)) = last_error {
                if *code != 0 {
                    sections.push(Section::Spacer);
                    sections.push(Section::Text {
                        label: format!("HTTP {code}"),
                        value: msg.clone(),
                    });
                }
            }
            sections
        }
    }
}

fn anthropic_sections(
    s: &crate::usage::AnthropicSnapshot,
    now: DateTime<Utc>,
    tol: u32,
) -> Vec<Section> {
    let mut v = vec![Section::Title {
        left: format!("Claude {}", s.plan),
        right: None,
    }];

    push_window(&mut v, "Session (5h)", &s.session, now, tol, true);
    push_window(&mut v, "Weekly (7d)", &s.weekly, now, tol, true);
    if let Some(w) = &s.sonnet {
        push_window(&mut v, "Sonnet only", w, now, tol, false);
    }
    if let Some(e) = &s.extra {
        v.push(Section::Spacer);
        let pct = e.percent().clamp(0, 100) as u16;
        v.push(Section::Metric {
            label: "Extra usage".into(),
            pct,
            severity: severity_for(pct as i32),
            value_label: format!("{} of {}", e.spent.fmt_dollars(), e.limit.fmt_dollars()),
            footnote: format!("{}% of monthly limit consumed", pct),
        });
    }
    v
}

fn openai_sections(s: &crate::usage::OpenAiSnapshot, now: DateTime<Utc>, tol: u32) -> Vec<Section> {
    let mut v = vec![Section::Title {
        left: s.plan.clone(),
        right: None,
    }];
    push_window(&mut v, "Codex 5h", &s.session, now, tol, true);
    push_window(&mut v, "Codex weekly", &s.weekly, now, tol, true);
    if let Some(cr) = &s.code_review {
        push_window(&mut v, "Code review", cr, now, tol, false);
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
    v
}

fn zai_sections(s: &crate::usage::ZaiSnapshot, now: DateTime<Utc>) -> Vec<Section> {
    let mut v = vec![Section::Title {
        left: s.plan.clone(),
        right: None,
    }];
    if let Some(w) = &s.session {
        push_window(&mut v, "Session (5h)", w, now, 5, false);
    }
    if let Some(w) = &s.weekly {
        push_window(&mut v, "Weekly", w, now, 5, false);
    }
    if let Some(w) = &s.mcp {
        push_window(&mut v, "MCP tools (monthly)", w, now, 5, false);
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

fn openrouter_sections(s: &crate::usage::OpenRouterSnapshot) -> Vec<Section> {
    let mut v = vec![Section::Title {
        left: s.label.clone(),
        right: None,
    }];
    let pct = s.consumed_pct().clamp(0, 100) as u16;
    v.push(Section::Spacer);
    v.push(Section::Metric {
        label: "Credit balance".into(),
        pct,
        severity: severity_for(pct as i32),
        value_label: format!("${:.2}", s.balance()),
        footnote: format!(
            "${:.2} of ${:.2} used ({pct}%)",
            s.total_usage, s.total_credits
        ),
    });
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
            body: vec![format!("${:.2} of ${:.2} remaining", rem, limit)],
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

fn push_window(
    sections: &mut Vec<Section>,
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
    sections.push(Section::Metric {
        label: label.into(),
        pct,
        severity: severity_for(pct as i32),
        value_label: format!("{pct}%"),
        footnote,
    });
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
        render_section(f, chunks[i], theme, s);
    }
    if pin_last {
        render_section(f, chunks[chunks.len() - 1], theme, sections.last().unwrap());
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

fn render_section(f: &mut Frame, area: Rect, theme: &Theme, s: &Section) {
    let accent = parse_hex(&theme.blue).unwrap_or(Color::Cyan);
    let dim = parse_hex(&theme.dim).unwrap_or(Color::DarkGray);
    let fg = parse_hex(&theme.fg).unwrap_or(Color::White);

    match s {
        Section::Title { left, right } => {
            // Left: bold accent-colored plan/vendor label. Right: dim-styled
            // "Updated HH:MM:SS" pinned to the right edge of the title row.
            let left_line = Line::from(Span::styled(
                format!("  {left}"),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ));
            f.render_widget(Paragraph::new(left_line), area);
            if let Some(rt) = right {
                let right_line = Line::from(Span::styled(
                    format!("{rt}  "),
                    Style::default().fg(dim),
                ))
                .right_aligned();
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
            label,
            *pct,
            *severity,
            value_label,
            footnote,
        ),
        Section::Text { label, value } => {
            let mut spans = Vec::new();
            if !label.is_empty() {
                spans.push(Span::styled(
                    format!("  {label}  "),
                    Style::default().fg(fg).add_modifier(Modifier::BOLD),
                ));
            }
            spans.push(Span::styled(value.clone(), Style::default().fg(dim)));
            f.render_widget(Paragraph::new(Line::from(spans)), area);
        }
        Section::Block { label, body } => render_block(f, area, theme, label, body),
        Section::Spacer => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn render_metric(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    label: &str,
    pct: u16,
    severity: PaceSeverity,
    value_label: &str,
    footnote: &str,
) {
    let fg = parse_hex(&theme.fg).unwrap_or(Color::White);
    let dim = parse_hex(&theme.dim).unwrap_or(Color::DarkGray);
    let bar_color = match severity {
        PaceSeverity::Low => parse_hex(&theme.green),
        PaceSeverity::Mid => parse_hex(&theme.yellow),
        PaceSeverity::High => parse_hex(&theme.orange),
        PaceSeverity::Critical => parse_hex(&theme.red),
    }
    .unwrap_or(Color::Green);
    let bar_empty = parse_hex(&theme.bar_empty).unwrap_or(Color::Black);

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
        Style::default().fg(fg).add_modifier(Modifier::BOLD),
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
    let gauge = Gauge::default()
        .block(Block::default())
        .gauge_style(Style::default().fg(bar_color).bg(bar_empty))
        .percent(pct)
        .label("");
    f.render_widget(gauge, gauge_area);
    let value = Paragraph::new(Line::from(Span::styled(
        value_label.to_string(),
        Style::default().fg(bar_color).add_modifier(Modifier::BOLD),
    )));
    f.render_widget(value, value_area);

    // Row 3: footnote (dim)
    let foot = Line::from(Span::styled(
        format!("    {footnote}"),
        Style::default().fg(dim),
    ));
    f.render_widget(Paragraph::new(foot), inner[2]);
}

fn render_block(f: &mut Frame, area: Rect, theme: &Theme, label: &str, body: &[String]) {
    let fg = parse_hex(&theme.fg).unwrap_or(Color::White);
    let dim = parse_hex(&theme.dim).unwrap_or(Color::DarkGray);

    let mut lines = vec![Line::from(Span::styled(
        format!("  {label}"),
        Style::default().fg(fg).add_modifier(Modifier::BOLD),
    ))];
    for b in body {
        lines.push(Line::from(Span::styled(
            format!("    {b}"),
            Style::default().fg(dim),
        )));
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn parse_hex(s: &str) -> Option<Color> {
    let (r, g, b) = crate::theme::parse_hex_rgb(s)?;
    Some(Color::Rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{
        AnthropicSnapshot, Cents, ExtraUsage, OpenAiCredits, OpenAiSnapshot, OpenAiSource,
        OpenRouterSnapshot, UsageWindow, ZaiSnapshot,
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
            extra: Some(ExtraUsage {
                limit: Cents(5000),
                spent: Cents(250),
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
            session: UsageWindow {
                utilization_pct: 1,
                resets_at: None,
                window_duration: chrono::Duration::hours(5),
            },
            weekly: UsageWindow {
                utilization_pct: 0,
                resets_at: None,
                window_duration: chrono::Duration::days(7),
            },
            code_review: None,
            credits: Some(OpenAiCredits {
                balance: "$5.00".into(),
                has_credits: true,
                unlimited: false,
                approx_local_messages: Some((100, 200)),
                approx_cloud_messages: Some((30, 50)),
            }),
            source: OpenAiSource::CodexOauth,
        };
        let sections = sections_for(&ready(VendorSnapshot::Openai(snap)), now(), 5);
        assert!(
            sections
                .iter()
                .any(|s| matches!(s, Section::Block { label, .. } if label == "Credits"))
        );
    }
}
