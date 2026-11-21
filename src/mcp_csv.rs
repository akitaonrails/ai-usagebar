//! CSV projection of the `get_usage` report for MCP clients.
//!
//! An agent that polls quota on every turn pays for the whole versioned report
//! — `sections` repeats every gauge, and error bodies can be whole HTML pages —
//! while it only decides on a handful of fields. This module turns the same
//! JSON document the collector already produced into one compact row per
//! gauge, so filtering, caching, and error handling stay in `report.rs`.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::display::sanitize_untrusted_line;

const USAGE_CSV_HEADER: [&str; 6] = ["id", "label", "percent", "reset_at", "severity", "note"];

/// Block heading under which a provider lists models it cannot dispatch to
/// (`tui::panels`), carried into the report's `sections` unchanged.
const UNAVAILABLE_SECTION_LABEL: &str = "Unavailable";

type UsageCsvRow = [String; 6];

/// The subset of the report contract (`report::json_rows`) the CSV reads.
/// Unknown fields are ignored, as the README asks of every consumer.
#[derive(Debug, Deserialize)]
struct UsageCsvReport {
    entries: Vec<UsageCsvEntry>,
}

#[derive(Debug, Deserialize)]
struct UsageCsvEntry {
    id: String,
    status: String,
    #[serde(default)]
    stale: bool,
    #[serde(default)]
    reset_credits: Option<UsageCsvResetCredits>,
    #[serde(default)]
    metrics: Vec<UsageCsvMetric>,
    #[serde(default)]
    sections: Vec<UsageCsvSection>,
}

#[derive(Debug, Deserialize)]
struct UsageCsvMetric {
    label: String,
    percent: u16,
    severity: String,
    #[serde(default)]
    reset_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct UsageCsvSection {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    body: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct UsageCsvResetCredits {
    available: u32,
}

/// Project a `usage --json` document into CSV text: the header
/// `id,label,percent,reset_at,severity,note`, then one row per gauge, or one
/// row for an entry without gauges, its `status` in the `severity` column.
/// Error messages are left out on purpose — `error` there already says the
/// entry failed, and the message can be a whole provider HTML page.
///
/// Fails when the document is not the report shape, so a caller can report an
/// error instead of handing back an empty table.
///
/// ```text
/// let csv = usage_report_csv(r#"{"entries":[{"id":"custom:example","status":"ready"}]}"#)?;
/// assert_eq!(csv, "id,label,percent,reset_at,severity,note\ncustom:example,,,,ready,\n");
/// ```
pub(crate) fn usage_report_csv(document: &str) -> Result<String, serde_json::Error> {
    let report: UsageCsvReport = serde_json::from_str(document)?;
    let mut csv = csv_line(&USAGE_CSV_HEADER.map(str::to_owned));
    for row in report.entries.iter().flat_map(entry_rows) {
        csv.push_str(&csv_line(&row));
    }
    Ok(csv)
}

fn entry_rows(entry: &UsageCsvEntry) -> Vec<UsageCsvRow> {
    let note = entry_note(entry);
    if entry.metrics.is_empty() {
        let status = entry.status.clone();
        return vec![[
            entry.id.clone(),
            String::new(),
            String::new(),
            String::new(),
            status,
            note,
        ]];
    }
    // The note describes the entry, not a gauge: repeating it would only cost
    // the tokens this format exists to save.
    let notes = std::iter::once(note).chain(std::iter::repeat(String::new()));
    entry
        .metrics
        .iter()
        .zip(notes)
        .map(|(metric, note)| metric_row(&entry.id, metric, note))
        .collect()
}

fn metric_row(id: &str, metric: &UsageCsvMetric, note: String) -> UsageCsvRow {
    [
        id.to_owned(),
        metric.label.clone(),
        metric.percent.to_string(),
        metric.reset_at.map(reset_to_the_minute).unwrap_or_default(),
        metric.severity.clone(),
        note,
    ]
}

/// UTC, minute precision: a reset is planned around in minutes, and seconds
/// would only add tokens.
fn reset_to_the_minute(reset_at: DateTime<Utc>) -> String {
    reset_at.format("%Y-%m-%dT%H:%MZ").to_string()
}

fn entry_note(entry: &UsageCsvEntry) -> String {
    let mut parts: Vec<String> = Vec::new();
    if entry.stale {
        parts.push("stale".to_owned());
    }
    let unavailable = unavailable_models(entry);
    if !unavailable.is_empty() {
        parts.push(format!("unavailable: {}", unavailable.join(", ")));
    }
    let credits = entry.reset_credits.as_ref().map_or(0, |c| c.available);
    if credits > 0 {
        parts.push(format!("reset_credits={credits}"));
    }
    parts.join("; ")
}

fn unavailable_models(entry: &UsageCsvEntry) -> Vec<String> {
    entry
        .sections
        .iter()
        .filter(|section| section.label.as_deref() == Some(UNAVAILABLE_SECTION_LABEL))
        .flat_map(|section| section.body.iter())
        .map(|line| sanitize_untrusted_line(line))
        .collect()
}

fn csv_line(row: &[String]) -> String {
    let cells: Vec<String> = row.iter().map(|cell| csv_cell(cell)).collect();
    format!("{}\n", cells.join(","))
}

/// Every cell is sanitized at this sink, the way the JSON error path is, and
/// as one line: a line-oriented consumer should never meet a quoted newline.
fn csv_cell(raw: &str) -> String {
    rfc4180_quote(&sanitize_untrusted_line(raw))
}

/// Quote one already-sanitized cell per RFC 4180.
fn rfc4180_quote(field: &str) -> String {
    if !field.contains([',', '"', '\r', '\n']) {
        return field.to_owned();
    }
    format!("\"{}\"", field.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(fields: serde_json::Value) -> String {
        let mut entry = serde_json::json!({
            "id": "anthropic@work",
            "status": "ready",
            "stale": false,
            "reset_credits": null,
            "metrics": [],
            "sections": []
        });
        for (key, value) in fields.as_object().expect("object fixture") {
            entry[key] = value.clone();
        }
        serde_json::json!({"schema_version": 1, "entries": [entry]}).to_string()
    }

    #[test]
    fn quoting_leaves_plain_fields_alone_and_wraps_special_characters() {
        assert_eq!(rfc4180_quote("Session (5h)"), "Session (5h)");
        assert_eq!(rfc4180_quote("a,b"), "\"a,b\"");
        assert_eq!(rfc4180_quote("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(rfc4180_quote("line\nbreak"), "\"line\nbreak\"");
        assert_eq!(rfc4180_quote("carriage\rreturn"), "\"carriage\rreturn\"");
    }

    #[test]
    fn reset_is_converted_to_utc_and_truncated_to_the_minute() {
        let document = entry(serde_json::json!({
            "metrics": [{
                "label": "Session (5h)",
                "percent": 42,
                "severity": "low",
                "reset_at": "2026-11-19T01:59:59.999-03:00"
            }]
        }));
        assert_eq!(
            usage_report_csv(&document).unwrap(),
            "id,label,percent,reset_at,severity,note\n\
             anthropic@work,Session (5h),42,2026-11-19T04:59Z,low,\n"
        );
    }

    #[test]
    fn note_parts_keep_a_fixed_order() {
        let document = entry(serde_json::json!({
            "stale": true,
            "reset_credits": {"available": 1, "credits": []},
            "sections": [
                {"type": "spacer"},
                {"type": "block", "label": "Unavailable", "body": ["model-a — at capacity"]}
            ]
        }));
        assert_eq!(
            usage_report_csv(&document).unwrap(),
            "id,label,percent,reset_at,severity,note\n\
             anthropic@work,,,,ready,stale; unavailable: model-a — at capacity; reset_credits=1\n"
        );
    }

    #[test]
    fn control_characters_in_provider_text_are_removed_before_quoting() {
        let document = entry(serde_json::json!({
            "metrics": [{
                "label": "Opus\u{1b}[31m\nweekly",
                "percent": 5,
                "severity": "low",
                "reset_at": null
            }]
        }));
        assert_eq!(
            usage_report_csv(&document).unwrap(),
            "id,label,percent,reset_at,severity,note\n\
             anthropic@work,Opus[31m weekly,5,,low,\n"
        );
    }

    #[test]
    fn a_document_without_entries_is_rejected() {
        assert!(usage_report_csv(r#"{"schema_version":1}"#).is_err());
        assert!(usage_report_csv(r#"{"entries":{}}"#).is_err());
        assert!(usage_report_csv("not json").is_err());
    }
}
