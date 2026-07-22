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
use crate::format::local_time_hms;
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
                VendorSnapshot::AnthropicApi(s) => anthropic_api_sections(s),
                VendorSnapshot::Openai(s) => openai_sections(s, now, pace_tolerance),
                VendorSnapshot::Zai(s) => zai_sections(s, now),
                VendorSnapshot::Openrouter(s) => openrouter_sections(s),
                VendorSnapshot::Deepseek(s) => deepseek_sections(s),
                VendorSnapshot::Kimi(s) => kimi_sections(s, now, pace_tolerance),
                VendorSnapshot::Kilo(s) => kilo_sections(s),
                VendorSnapshot::Novita(s) => novita_sections(s),
                VendorSnapshot::Moonshot(s) => moonshot_sections(s),
                VendorSnapshot::Grok(s) => grok_sections(s),
                VendorSnapshot::Antigravity(s) => antigravity_sections(s, now),
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
            if let Some((label, msg)) = warning_label(snapshot, last_error) {
                sections.push(Section::Spacer);
                sections.push(Section::Text { label, value: msg });
            }
            sections
        }
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

fn anthropic_api_sections(s: &crate::usage::AnthropicApiSnapshot) -> Vec<Section> {
    let mut v = vec![Section::Title {
        left: "Anthropic API".into(),
        right: None,
    }];
    match (s.limit.filter(|l| *l > 0.0), s.pct()) {
        (Some(limit), Some(pct)) => {
            let p = pct.clamp(0, 100) as u16;
            v.push(Section::Metric {
                label: "Spend (mo)".into(),
                pct: p,
                severity: severity_for(pct),
                value_label: format!("${:.2} of ${:.0}", s.spent, limit),
                footnote: format!("{pct}% of monthly limit"),
            });
        }
        _ => {
            v.push(Section::Text {
                label: "Spend (mo)".into(),
                value: format!("${:.2}", s.spent),
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

/// Antigravity holds two independent pools (Gemini, Claude & GPT OSS), each
/// with a 5-hour and a weekly window. Grouped by window type so the two pools
/// sit side by side, matching the GNOME dropdown.
fn antigravity_sections(s: &crate::usage::AntigravitySnapshot, now: DateTime<Utc>) -> Vec<Section> {
    use crate::antigravity::vendor::{GROUP_PRIMARY, GROUP_THIRD_PARTY};

    let mut v = vec![Section::Title {
        left: s.plan.clone(),
        right: None,
    }];
    for (heading, primary, third_party) in [
        ("Session", &s.session, s.third_party_session.as_ref()),
        ("Weekly", &s.weekly, s.third_party_weekly.as_ref()),
    ] {
        v.push(Section::Spacer);
        v.push(Section::Text {
            label: heading.into(),
            value: String::new(),
        });
        push_window(&mut v, GROUP_PRIMARY, primary, now, 5, false);
        if let Some(w) = third_party {
            push_window(&mut v, GROUP_THIRD_PARTY, w, now, 5, false);
        }
    }
    v
}

fn kilo_sections(s: &crate::usage::KiloSnapshot) -> Vec<Section> {
    vec![
        Section::Title {
            left: s.label.clone(),
            right: None,
        },
        Section::Spacer,
        Section::Text {
            label: "Balance".into(),
            value: format!("${:.2}", s.balance),
        },
    ]
}

fn novita_sections(s: &crate::usage::NovitaSnapshot) -> Vec<Section> {
    let mut v = vec![
        Section::Title {
            left: "Novita".into(),
            right: None,
        },
        Section::Spacer,
        Section::Text {
            label: "Balance".into(),
            value: format!("${:.2}", s.available),
        },
        Section::Block {
            label: "Breakdown".into(),
            body: vec![format!(
                "top-up ${:.2} · credit limit ${:.2}",
                s.cash, s.credit_limit
            )],
        },
    ];
    if s.outstanding > 0.0 {
        v.push(Section::Spacer);
        v.push(Section::Block {
            label: "Owed".into(),
            body: vec![format!("${:.2}", s.outstanding)],
        });
    }
    v
}

fn moonshot_sections(s: &crate::usage::MoonshotSnapshot) -> Vec<Section> {
    let cur = &s.currency;
    let fmt = |v: f64| match cur.as_str() {
        "USD" => format!("${v:.2}"),
        "CNY" => format!("¥{v:.2}"),
        _ => format!("{v:.2} {cur}"),
    };
    vec![
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
    ]
}

fn grok_sections(s: &crate::usage::GrokSnapshot) -> Vec<Section> {
    vec![
        Section::Title {
            left: "Grok (xAI)".into(),
            right: None,
        },
        Section::Spacer,
        Section::Text {
            label: "Prepaid balance".into(),
            value: format!("${:.2}", s.balance),
        },
    ]
}

fn deepseek_sections(s: &crate::usage::DeepseekSnapshot) -> Vec<Section> {
    let currency = &s.currency;
    let fmt = |v: f64| match currency.as_str() {
        "USD" => format!("${v:.2}"),
        "CNY" => format!("¥{v:.2}"),
        _ => format!("{v:.2} {currency}"),
    };
    let avail = if s.is_available {
        "available"
    } else {
        "unavailable"
    };
    let mut v = vec![Section::Title {
        left: "DeepSeek".into(),
        right: None,
    }];
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

fn kimi_sections(s: &crate::usage::KimiSnapshot, now: DateTime<Utc>, _tol: u32) -> Vec<Section> {
    let plan = s.plan.as_deref().unwrap_or("Kimi");
    let mut v = vec![Section::Title {
        left: plan.into(),
        right: None,
    }];

    let weekly_pct = s.weekly_pct().clamp(0, 100) as u16;
    v.push(Section::Spacer);
    v.push(Section::Metric {
        label: "Weekly quota".into(),
        pct: weekly_pct,
        severity: severity_for(s.weekly_pct()),
        value_label: format!("{} / {}", s.weekly_used, s.weekly_limit),
        footnote: format!(
            "{} remaining · reset {}",
            s.weekly_remaining,
            countdown::format(s.weekly_reset_at, now)
        ),
    });

    if s.window_limit > 0 {
        let window_pct = s.window_pct().clamp(0, 100) as u16;
        v.push(Section::Spacer);
        v.push(Section::Metric {
            label: "Rolling window (5h)".into(),
            pct: window_pct,
            severity: severity_for(s.window_pct()),
            value_label: format!("{} / {}", s.window_used, s.window_limit),
            footnote: format!(
                "{} remaining · reset {}",
                s.window_remaining,
                countdown::format(s.window_reset_at, now)
            ),
        });
    }

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
        OpenAiSource, OpenRouterSnapshot, UsageWindow, ZaiSnapshot,
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
            scoped: vec![],
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

        let (weekly_value, weekly_footnote) = find_footnote("Weekly quota");
        assert_eq!(weekly_value, "26 / 100");
        assert!(weekly_footnote.contains("74 remaining"));
        assert!(
            weekly_footnote.contains("4d 0h"),
            "weekly reset countdown: {weekly_footnote}"
        );
        assert!(!weekly_footnote.contains("2026-05-27T")); // not a raw RFC3339

        let (window_value, window_footnote) = find_footnote("Rolling window (5h)");
        assert_eq!(window_value, "15 / 100");
        assert!(window_footnote.contains("85 remaining"));
        assert!(
            window_footnote.contains("2h 00m"),
            "window reset countdown: {window_footnote}"
        );
        assert!(!window_footnote.contains("2026-05-23T14")); // not a raw RFC3339
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
