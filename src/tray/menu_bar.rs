//! Chart or starred-metric logo strip for the macOS status item.
//!
//! `StripContent` owns visible groups and their values; the report supplies
//! only the short-name fallback for providers without an embedded mark.

use serde_json::Value;

use super::strip::StripContent;

/// Artwork needed for the current status-item content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StatusItemContent {
    /// The static application icon is the empty-content fallback.
    AppIcon,
    /// Draw the usage bars.
    Chart,
    /// Draw provider logos and starred metric values.
    Logos,
}

/// Select status-item artwork, falling back to the app icon when content is empty.
pub(super) fn status_item_content(chart: bool, has_content: bool) -> StatusItemContent {
    if !has_content {
        StatusItemContent::AppIcon
    } else if chart {
        StatusItemContent::Chart
    } else {
        StatusItemContent::Logos
    }
}

/// One provider's visible logo (or short-name fallback) and starred values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LogoSegment {
    /// Lowercase provider slug used to find its embedded mark.
    pub(super) slug: String,
    /// Report short name, used only when its embedded mark cannot be drawn.
    pub(super) short_name: Option<String>,
    /// Non-empty metric values in star order; their count is the rendered line count.
    pub(super) values: Vec<String>,
}

/// Build logo segments from the same starred metric groups used by the chart.
pub(super) fn logo_segments(content: &StripContent, report: &Value) -> Vec<LogoSegment> {
    content
        .groups
        .iter()
        .filter_map(|(id, _, metrics)| {
            let values: Vec<String> = metrics
                .iter()
                .map(|metric| metric.value.trim())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .take(2)
                .collect();
            if values.is_empty() {
                return None;
            }
            let slug = id.split('@').next()?.to_ascii_lowercase();
            if slug.is_empty() {
                return None;
            }
            let short_name = short_name_for(report, id);
            Some(LogoSegment {
                slug,
                short_name,
                values,
            })
        })
        .collect()
}

/// Build tooltip lines for the starred groups, or the app name when none have values.
pub(super) fn tooltip(content: &StripContent) -> String {
    let lines: Vec<String> = content
        .groups
        .iter()
        .filter_map(|(_, name, metrics)| {
            let values: Vec<&str> = metrics
                .iter()
                .map(|metric| metric.value.trim())
                .filter(|value| !value.is_empty())
                .collect();
            if values.is_empty() {
                return None;
            }
            Some(format!("{} · {}", safe_text(name), values.join(" ")))
        })
        .collect();
    if lines.is_empty() {
        "AI Usage".into()
    } else {
        lines.join("\n")
    }
}

fn short_name_for(report: &Value, id: &str) -> Option<String> {
    report
        .get("entries")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("id").and_then(Value::as_str) == Some(id))?
        .get("short_name")?
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(safe_text)
}

fn safe_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_modes_fall_back_to_static_app_icon() {
        assert_eq!(status_item_content(true, false), StatusItemContent::AppIcon);
        assert_eq!(
            status_item_content(false, false),
            StatusItemContent::AppIcon
        );
        assert_eq!(status_item_content(true, true), StatusItemContent::Chart);
        assert_eq!(status_item_content(false, true), StatusItemContent::Logos);
    }

    #[test]
    fn logo_segments_keep_one_star_value_and_use_star_order() {
        let content = StripContent {
            groups: vec![
                ("anthropic".into(), "Claude".into(), vec![metric("41%")]),
                (
                    "openai".into(),
                    "Codex".into(),
                    vec![metric("9%"), metric("5%")],
                ),
            ],
            bars: Vec::new(),
        };
        let report = json!({"entries":[]});

        let segments = logo_segments(&content, &report);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].slug, "anthropic");
        assert_eq!(segments[0].values, vec![String::from("41%")]);
        assert_eq!(segments[1].slug, "openai");
        assert_eq!(
            segments[1].values,
            vec![String::from("9%"), String::from("5%")]
        );
    }

    #[test]
    fn logo_segments_skip_groups_without_values() {
        let content = StripContent {
            groups: vec![
                ("cursor".into(), "Cursor".into(), vec![metric("  ")]),
                ("zai".into(), "Z.AI".into(), vec![metric("12%")]),
            ],
            bars: Vec::new(),
        };

        let segments = logo_segments(&content, &json!({"entries":[]}));
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].slug, "zai");
        assert_eq!(segments[0].values, vec![String::from("12%")]);
    }

    #[test]
    fn content_without_metric_values_stays_empty_in_both_modes() {
        let report = json!({"entries":[
            {"id":"claude", "display_name":"Claude", "sections":[
                {"type":"metric", "label":"Weekly"}
            ]}
        ]});
        let content = super::super::strip::content_from_payload(
            &report,
            &super::super::strip::Stars::new(),
            &[],
        );

        assert!(content.groups.is_empty());
        assert!(content.bars.is_empty());
        assert!(logo_segments(&content, &report).is_empty());
    }

    #[test]
    fn logo_segments_use_report_short_name_when_mark_is_unknown() {
        let content = StripContent {
            groups: vec![(
                "unknown@work".into(),
                "Unknown Provider".into(),
                vec![metric("8%")],
            )],
            bars: Vec::new(),
        };
        let report = json!({"entries":[
            {"id":"unknown@work", "short_name":"unk"}
        ]});

        let segments = logo_segments(&content, &report);
        assert_eq!(segments[0].slug, "unknown");
        assert!(super::super::marks::mark_svg(&segments[0].slug).is_none());
        assert_eq!(segments[0].short_name.as_deref(), Some("unk"));
    }

    #[test]
    fn tooltip_lists_each_starred_group_and_uses_generic_empty_text() {
        let content = StripContent {
            groups: vec![
                (
                    "anthropic".into(),
                    "Claude".into(),
                    vec![metric("41%"), metric("5%")],
                ),
                ("openai".into(), "Codex".into(), vec![metric("9%")]),
                ("cursor".into(), "Cursor".into(), vec![metric("")]),
            ],
            bars: Vec::new(),
        };

        assert_eq!(tooltip(&content), "Claude · 41% 5%\nCodex · 9%");
        assert_eq!(
            tooltip(&StripContent {
                groups: Vec::new(),
                bars: Vec::new(),
            }),
            "AI Usage"
        );
    }

    fn metric(value: &str) -> super::super::strip::StripMetric {
        super::super::strip::StripMetric {
            provider_id: String::new(),
            provider_name: String::new(),
            key: String::new(),
            label: String::new(),
            value: value.to_owned(),
            fraction: 0.0,
            bounded: true,
        }
    }
}
