//! Omarchy-style provider summary for the macOS status item.
//!
//! The report owns provider names and metric values. This module only chooses
//! the visible entry and quota window, so the native host can repaint without
//! asking the WebView to be open.

use serde_json::Value;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UsageWindow {
    #[default]
    Auto,
    Session,
    Weekly,
    Monthly,
}

impl UsageWindow {
    pub fn parse(value: &str) -> Self {
        match value {
            "session" => Self::Session,
            "weekly" => Self::Weekly,
            "monthly" => Self::Monthly,
            _ => Self::Auto,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Session => "session",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }
}

pub fn selected_id<'a>(payload: &'a Value, remembered: &'a str) -> Option<&'a str> {
    let entries = payload.get("entries")?.as_array()?;
    if entries.is_empty() {
        return None;
    }
    let has_id = |id: &str| {
        entries
            .iter()
            .any(|entry| entry.get("id").and_then(Value::as_str) == Some(id))
    };
    if !remembered.is_empty() && has_id(remembered) {
        return Some(remembered);
    }
    if let Some(primary) = payload.get("primary").and_then(Value::as_str) {
        if has_id(primary) {
            return Some(primary);
        }
        if let Some(entry) = entries.iter().find(|entry| {
            entry
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.split('@').next() == Some(primary))
        }) {
            return entry.get("id").and_then(Value::as_str);
        }
    }
    entries.first()?.get("id").and_then(Value::as_str)
}

pub fn next_id(payload: &Value, remembered: &str) -> Option<String> {
    let ids: Vec<&str> = payload
        .get("entries")?
        .as_array()?
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .collect();
    if ids.is_empty() {
        return None;
    }
    let current = selected_id(payload, remembered).unwrap_or(ids[0]);
    let index = ids.iter().position(|id| *id == current).unwrap_or(0);
    Some(ids[(index + 1) % ids.len()].to_owned())
}

pub fn title(
    payload: &Value,
    remembered: &str,
    show_all: bool,
    show_value: bool,
    window: UsageWindow,
) -> String {
    let Some(entries) = payload.get("entries").and_then(Value::as_array) else {
        return String::new();
    };
    let selected = selected_id(payload, remembered);
    let chips: Vec<String> = entries
        .iter()
        .filter(|entry| show_all || entry.get("id").and_then(Value::as_str) == selected)
        .map(|entry| chip(entry, show_value, window))
        .filter(|chip| !chip.is_empty())
        .collect();
    chips.join("   ")
}

pub fn tooltip(payload: &Value, remembered: &str, show_all: bool, window: UsageWindow) -> String {
    let Some(entries) = payload.get("entries").and_then(Value::as_array) else {
        return "AI Usage".into();
    };
    let selected = selected_id(payload, remembered);
    let lines: Vec<String> = entries
        .iter()
        .filter(|entry| show_all || entry.get("id").and_then(Value::as_str) == selected)
        .map(|entry| {
            let name = entry
                .get("display_name")
                .or_else(|| entry.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("AI Usage");
            let stale = if entry.get("stale").and_then(Value::as_bool) == Some(true) {
                " · cached"
            } else {
                ""
            };
            format!(
                "{} · {}{stale}",
                safe_text(name, 48),
                headline(entry, window)
            )
        })
        .collect();
    if lines.is_empty() {
        "AI Usage".into()
    } else {
        lines.join("\n")
    }
}

fn chip(entry: &Value, show_value: bool, window: UsageWindow) -> String {
    let id = entry.get("id").and_then(Value::as_str).unwrap_or("");
    let code = entry
        .get("short_name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| id.split('@').next().unwrap_or(""));
    let code = safe_text(code, 12);
    if code.is_empty() || !show_value {
        return code;
    }
    let summary = headline(entry, window);
    format!("{code} {summary}")
}

fn headline(entry: &Value, window: UsageWindow) -> String {
    if entry.get("status").and_then(Value::as_str) == Some("error")
        || entry
            .get("error")
            .and_then(Value::as_str)
            .is_some_and(|error| !error.is_empty())
    {
        return "!".into();
    }
    let sections = entry.get("sections").and_then(Value::as_array);
    let metrics: Vec<&Value> = sections
        .into_iter()
        .flatten()
        .filter(|section| section.get("type").and_then(Value::as_str) == Some("metric"))
        .collect();
    let candidates: Vec<&Value> = metrics
        .iter()
        .copied()
        .filter(|metric| matches_window(metric, window))
        .collect();
    let pool = if candidates.is_empty() {
        &metrics
    } else {
        &candidates
    };
    if let Some(metric) = pool.iter().copied().max_by(|a, b| {
        let a = a.get("percent").and_then(Value::as_f64).unwrap_or(0.0);
        let b = b.get("percent").and_then(Value::as_f64).unwrap_or(0.0);
        a.total_cmp(&b)
    }) {
        if metric.get("headline").and_then(Value::as_str) == Some("value")
            && let Some(value) = metric.get("value").and_then(Value::as_str)
            && !value.is_empty()
        {
            return safe_text(value, 24);
        }
        if let Some(percent) = metric.get("percent") {
            return format!("{percent}%");
        }
    }
    if let Some(sections) = sections {
        for section in sections {
            if section.get("type").and_then(Value::as_str) != Some("text") {
                continue;
            }
            let label = section
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            if ["balance", "available", "spend", "prepaid"]
                .iter()
                .any(|term| label.contains(term))
                && let Some(value) = section.get("value").and_then(Value::as_str)
                && !value.is_empty()
            {
                return safe_text(value, 24);
            }
        }
    }
    "Ready".into()
}

fn matches_window(metric: &Value, window: UsageWindow) -> bool {
    if window == UsageWindow::Auto {
        return true;
    }
    let seconds = metric.get("window_secs").and_then(Value::as_u64);
    if let Some(18_000 | 604_800) = seconds {
        return match window {
            UsageWindow::Session => seconds == Some(18_000),
            UsageWindow::Weekly => seconds == Some(604_800),
            _ => false,
        };
    }
    let label = metric
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    match window {
        UsageWindow::Session => ["5h", "5-hour", "session", "rolling"]
            .iter()
            .any(|term| label.contains(term)),
        UsageWindow::Weekly => ["week", "7d", "7-day"]
            .iter()
            .any(|term| label.contains(term)),
        UsageWindow::Monthly => ["month", "30d", "30-day", "spend (mo)"]
            .iter()
            .any(|term| label.contains(term)),
        UsageWindow::Auto => true,
    }
}

fn safe_text(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn remembers_a_provider_and_cycles_in_report_order() {
        let report = json!({"primary":"claude", "entries":[
            {"id":"claude", "short_name":"cld"},
            {"id":"openai@work", "short_name":"cdx"},
            {"id":"cursor", "short_name":"cur"}
        ]});
        assert_eq!(selected_id(&report, "openai@work"), Some("openai@work"));
        assert_eq!(next_id(&report, "openai@work"), Some("cursor".into()));
        assert_eq!(next_id(&report, "cursor"), Some("claude".into()));
        assert_eq!(selected_id(&report, "missing"), Some("claude"));
    }

    #[test]
    fn pinned_window_falls_back_and_value_headlines_stay_values() {
        let report = json!({"primary":"openai", "entries":[
            {"id":"openai", "short_name":"cdx", "sections":[
                {"type":"metric", "label":"5h", "percent":20, "window_secs":18000},
                {"type":"metric", "label":"Weekly", "percent":80, "window_secs":604800}
            ]},
            {"id":"openrouter", "short_name":"opr", "sections":[
                {"type":"metric", "label":"Balance", "percent":40, "headline":"value", "value":"$12.50"}
            ]}
        ]});
        assert_eq!(
            title(&report, "", false, true, UsageWindow::Auto),
            "cdx 80%"
        );
        assert_eq!(
            title(&report, "", false, true, UsageWindow::Session),
            "cdx 20%"
        );
        assert_eq!(
            title(&report, "openrouter", false, true, UsageWindow::Weekly),
            "opr $12.50"
        );
        assert_eq!(
            title(&report, "", true, true, UsageWindow::Auto),
            "cdx 80%   opr $12.50"
        );
    }
}
