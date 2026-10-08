//! Menu-bar strip: starred metrics, OpenUsage bar geometry, and a template
//! RGBA glyph. Pure JSON in, pixels out — no AppKit, no HWND.

use std::collections::BTreeMap;

use serde_json::Value;

/// At most two starred metrics per provider, matching OpenUsage.
pub const MAX_STARS_PER_PROVIDER: usize = 2;
/// Compact Bars glyph shows at most four bounded metrics.
pub const MAX_BARS: usize = 4;
/// Logical size of the Bars glyph, matching NSStatusItem's 18 pt slot.
pub const BARS_POINT_SIDE: u32 = 18;
/// Pixel size of the Bars glyph at 2× (18 pt). Used by hosts that can only
/// ship one raster; macOS paints 1×/2×/3× from [`BARS_POINT_SIDE`].
pub const BARS_PIXEL_SIDE: u32 = BARS_POINT_SIDE * 2;
/// Backing scales packed into the macOS template image so a 1× external
/// and a 2×/3× Retina each get a native raster instead of a downsample.
#[cfg_attr(not(test), allow(dead_code))]
pub const BARS_SCALES: [u32; 3] = [1, 2, 3];

/// How the status item renders starred metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripStyle {
    Text,
    Bars,
}

impl StripStyle {
    pub fn parse(name: &str) -> Self {
        if name.eq_ignore_ascii_case("text") {
            Self::Text
        } else {
            Self::Bars
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Bars => "bars",
        }
    }
}

/// Stars the popover persists: provider id → metric keys, in star order.
pub type Stars = BTreeMap<String, Vec<String>>;

/// Metrics hidden in the popover's Customize: provider id → metric keys.
pub type HiddenRows = BTreeMap<String, Vec<String>>;

/// One resolved starred metric, ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct StripMetric {
    pub provider_id: String,
    pub provider_name: String,
    pub key: String,
    pub label: String,
    pub value: String,
    /// 0..=1 fill for bounded metrics (used fraction).
    pub fraction: f64,
    pub bounded: bool,
    /// The remaining share as text (`82%`) for a percentage row, the way the
    /// popover's Left reading shows it; `None` for a value headline or a row
    /// with no percent, which read the same either way.
    pub left_value: Option<String>,
    /// The report named this metric's own figure as its headline
    /// (`headline = "value"`, a prepaid balance): a figure, not a quota
    /// window, so it never wins the highest-window race the Quattro chip runs.
    pub value_headline: bool,
    /// Under a group heading (SuperGrok's product slices, the Claude entry's
    /// CLI sessions): a breakdown, not a quota window of its own.
    pub grouped: bool,
}

/// One provider's visible metrics: entry id, display name, and metrics.
pub type StripGroup = (String, String, Vec<StripMetric>);

/// Resolved strip contents. `groups` drives Text; `bars` drives Bars.
#[derive(Debug, Clone, PartialEq)]
pub struct StripContent {
    pub groups: Vec<StripGroup>,
    pub bars: Vec<StripMetric>,
}

impl StripContent {
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// One-line Text fallback when we cannot paint stacked type (Windows
    /// NotifyIcon, or a host that only has `set_title`).
    pub fn title_line(&self) -> String {
        let mut parts = Vec::new();
        for (_, name, metrics) in &self.groups {
            let values: Vec<&str> = metrics.iter().map(|m| m.value.as_str()).collect();
            if values.is_empty() {
                continue;
            }
            parts.push(format!("{name} {}", values.join(" ")));
        }
        parts.join("   ")
    }
}

/// Fill geometry for one bar of the compact glyph. A 1:1 port of OpenUsage
/// `MenuBarBarGeometry`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarFill {
    pub fill_w: f64,
    pub remainder_w: f64,
    pub divider_x: Option<f64>,
}

/// Quantize near-full (0.7–1.0) bars by remainder in 15% steps, so a
/// nearly-full bar still leaves a visible tail instead of reading as 100%.
pub fn visual_fraction(fraction: f64) -> f64 {
    if !fraction.is_finite() {
        return 0.0;
    }
    let clamped = fraction.clamp(0.0, 1.0);
    if clamped > 0.7 && clamped < 1.0 {
        let remainder = 1.0 - clamped;
        let quantized = ((remainder / 0.15).ceil() * 0.15).min(1.0);
        (1.0 - quantized).max(0.0)
    } else {
        clamped
    }
}

pub fn bar_fill(track_w: f64, fraction: f64) -> BarFill {
    if !fraction.is_finite() {
        return BarFill {
            fill_w: 0.0,
            remainder_w: 0.0,
            divider_x: None,
        };
    }
    if fraction <= 0.0 {
        // An empty gauge is still a gauge: the whole track draws at the
        // remainder's strength, so a 0% row reads as an empty capsule — like
        // the popover's meter — instead of vanishing against the menu bar
        // (its bare 0.16-alpha track was all but invisible there).
        return BarFill {
            fill_w: 0.0,
            remainder_w: track_w,
            divider_x: Some(0.0),
        };
    }
    let visual = visual_fraction(fraction);
    if visual >= 1.0 {
        return BarFill {
            fill_w: track_w,
            remainder_w: 0.0,
            divider_x: None,
        };
    }
    let min_visible = (track_w * 0.2).round().max(4.0);
    let max_fill_w = (track_w - min_visible).max(1.0);
    let fill_w = (track_w * visual).round().clamp(1.0, max_fill_w);
    let true_remainder = track_w - fill_w;
    let remainder_w = true_remainder.max(min_visible).min(track_w - 1.0);
    BarFill {
        fill_w,
        remainder_w,
        divider_x: Some(track_w - remainder_w),
    }
}

/// Parse `{style, stars, order}` from the popover's `strip` IPC.
/// Icon style is locked to Bars; `style` is ignored if present.
pub fn parse_strip_ipc(value: &Value) -> (StripStyle, Stars, Vec<String>) {
    let style = StripStyle::Bars;
    let mut stars = Stars::new();
    if let Some(map) = value.get("stars").and_then(Value::as_object) {
        for (id, keys) in map {
            let id = id.trim();
            if id.is_empty() {
                continue;
            }
            let mut list = Vec::new();
            if let Some(arr) = keys.as_array() {
                for key in arr {
                    let Some(key) = key.as_str() else { continue };
                    let key = key.trim();
                    if key.is_empty() || list.iter().any(|k| k == key) {
                        continue;
                    }
                    list.push(key.to_string());
                    if list.len() == MAX_STARS_PER_PROVIDER {
                        break;
                    }
                }
            }
            if !list.is_empty() {
                stars.insert(id.to_string(), list);
            }
        }
    }
    let mut order = Vec::new();
    if let Some(arr) = value.get("order").and_then(Value::as_array) {
        for id in arr {
            let Some(id) = id.as_str() else { continue };
            let id = id.trim();
            if id.is_empty() || order.iter().any(|existing| existing == id) {
                continue;
            }
            order.push(id.to_string());
        }
    }
    (style, stars, order)
}

/// Parse `hidden_rows` from the popover's `strip` IPC: the metric keys the
/// user switched off, per provider. Missing or malformed means nothing is
/// hidden, the menu bar's behavior before the popover reports any.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub fn parse_hidden_rows(value: &Value) -> HiddenRows {
    let mut hidden = HiddenRows::new();
    let Some(map) = value.get("hidden_rows").and_then(Value::as_object) else {
        return hidden;
    };
    for (id, keys) in map {
        let id = id.trim();
        let Some(keys) = keys.as_array() else {
            continue;
        };
        let mut list: Vec<String> = Vec::new();
        for key in keys.iter().filter_map(Value::as_str).map(str::trim) {
            if !key.is_empty() && !list.iter().any(|k| k == key) {
                list.push(key.to_string());
            }
        }
        if !id.is_empty() && !list.is_empty() {
            hidden.insert(id.to_string(), list);
        }
    }
    hidden
}

/// Resolve starred metrics from a host payload. `order` is the popover's
/// visible card order; empty means payload order. Missing stars fall back to
/// the first two bounded metrics of each ready entry so the glyph has
/// something to draw before the WebView sends its layout.
pub fn content_from_payload(payload: &Value, stars: &Stars, order: &[String]) -> StripContent {
    let Some(entries) = payload.get("entries").and_then(Value::as_array) else {
        return StripContent {
            groups: Vec::new(),
            bars: Vec::new(),
        };
    };
    let mut by_id = std::collections::HashMap::new();
    let mut payload_ids = Vec::new();
    for entry in entries {
        let Some(id) = entry.get("id").and_then(Value::as_str) else {
            continue;
        };
        payload_ids.push(id.to_string());
        by_id.insert(id.to_string(), entry);
    }
    let mut walk = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for id in order {
        if by_id.contains_key(id) && seen.insert(id.clone()) {
            walk.push(id.clone());
        }
    }
    for id in payload_ids {
        if seen.insert(id.clone()) {
            walk.push(id);
        }
    }
    let groups: Vec<StripGroup> = walk
        .into_iter()
        .filter_map(|id| {
            let entry = by_id.get(&id)?;
            let starred =
                (!stars.is_empty()).then(|| stars.get(&id).map(Vec::as_slice).unwrap_or_default());
            entry_group(&id, entry, starred)
        })
        .collect();
    let bars: Vec<StripMetric> = groups
        .iter()
        .flat_map(|(_, _, metrics)| metrics.iter().cloned())
        .filter(|m| m.bounded)
        .take(MAX_BARS)
        .collect();
    StripContent { groups, bars }
}

/// Every quota window of one payload entry that has a value, stars aside,
/// the set the Quattro bar's `selectMetric` picks from: grouped rows only
/// when the entry has nothing else. The Quattro look draws its chip from this,
/// so it agrees with the popover tab that selects it, whichever window is
/// the busiest (Z.AI's third, monthly MCP window counts like its 5h and
/// weekly ones). A metric whose key is in `hidden` (switched off in the
/// popover's Customize) is left out, as the popover tab leaves it out; with
/// every metric hidden the entry has nothing to show. Only the macOS menu bar
/// draws the Quattro look, so other hosts compile it unused.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub fn quota_group(payload: &Value, id: &str, hidden: &[String]) -> Option<StripGroup> {
    let entry = payload
        .get("entries")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("id").and_then(Value::as_str) == Some(id))?;
    if entry.get("status").and_then(Value::as_str) == Some("error") {
        return None;
    }
    let name = entry_name(entry, id);
    let shown: Vec<StripMetric> = metrics_for_entry(entry, id, &name)
        .into_iter()
        .filter(|m| !m.value.trim().is_empty() && !hidden.contains(&m.key))
        .collect();
    let windows: Vec<StripMetric> = shown.iter().filter(|m| !m.grouped).cloned().collect();
    let metrics = if windows.is_empty() { shown } else { windows };
    (!metrics.is_empty()).then(|| (id.to_string(), name, metrics))
}

fn entry_name(entry: &Value, id: &str) -> String {
    entry
        .get("display_name")
        .or_else(|| entry.get("name"))
        .and_then(Value::as_str)
        .unwrap_or(id)
        .to_string()
}

/// Resolve one entry's visible metrics. `starred` is the provider's star
/// keys, or `None` before any star exists, which picks the first bounded
/// metrics. `None` for an errored entry or one with nothing to show.
fn entry_group(id: &str, entry: &Value, starred: Option<&[String]>) -> Option<StripGroup> {
    if entry.get("status").and_then(Value::as_str) == Some("error") {
        return None;
    }
    let name = entry_name(entry, id);
    let metrics = metrics_for_entry(entry, id, &name);
    let wanted: Vec<String> = match starred {
        Some(keys) => keys.to_vec(),
        None => metrics
            .iter()
            .filter(|m| m.bounded)
            .take(MAX_STARS_PER_PROVIDER)
            .map(|m| m.key.clone())
            .collect(),
    };
    let picked: Vec<StripMetric> = wanted
        .iter()
        .filter_map(|key| {
            metrics
                .iter()
                .find(|m| &m.key == key && !m.value.trim().is_empty())
                .cloned()
        })
        .collect();
    (!picked.is_empty()).then(|| (id.to_string(), name, picked))
}

fn metrics_for_entry(entry: &Value, id: &str, name: &str) -> Vec<StripMetric> {
    let Some(sections) = entry.get("sections").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut group = String::new();
    let mut rows = Vec::new();
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for section in sections {
        let kind = section.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "text" {
            let label = section.get("label").and_then(Value::as_str).unwrap_or("");
            let value = section.get("value").and_then(Value::as_str).unwrap_or("");
            if !label.is_empty() && value.is_empty() {
                group = label.to_string();
            }
            continue;
        }
        if kind != "metric" {
            continue;
        }
        let raw_label = section.get("label").and_then(Value::as_str).unwrap_or("");
        // A metric can name its own group in the report (SuperGrok's product
        // slices, the Claude entry's CLI-session rows); that field wins over
        // the positional heading in effect, mirroring the popover's
        // `projectCards` so both derive the same starred-metric key.
        let field_group = section.get("group").and_then(Value::as_str).unwrap_or("");
        let effective_group: &str = if field_group.is_empty() {
            group.as_str()
        } else {
            field_group
        };
        let label = metric_label(id, raw_label);
        let label = if effective_group.is_empty() {
            label
        } else {
            format!("{label} ({effective_group})")
        };
        let percent = section.get("percent").and_then(Value::as_f64);
        let value = section
            .get("value")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .or_else(|| percent.map(|percent| format!("{}%", percent.round() as i64)))
            .unwrap_or_default();
        let value_headline = section.get("headline").and_then(Value::as_str) == Some("value");
        let fraction = (percent.unwrap_or(0.0) / 100.0).clamp(0.0, 1.0);
        let left_value = percent
            .filter(|_| !value_headline)
            .map(|percent| format!("{}%", (100.0 - percent).max(0.0).round() as i64));
        let mut key = metric_key(id, raw_label, effective_group);
        let count = seen.entry(key.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            key = format!("{key} #{count}");
        }
        rows.push(StripMetric {
            provider_id: id.to_string(),
            provider_name: name.to_string(),
            key,
            label,
            value,
            fraction,
            bounded: true,
            left_value,
            value_headline,
            grouped: !effective_group.is_empty(),
        });
    }
    rows
}

/// The key a starred metric is stored under, as the popover derives it
/// (`metricRowKey` in windows/popover/src/model.js). A mismatch drops a starred
/// bar without a trace, so both sides test against
/// tests/fixtures/strip_metric_keys.json.
fn metric_key(entry_id: &str, raw_label: &str, group: &str) -> String {
    let label = metric_label(entry_id, raw_label);
    if group.is_empty() {
        format!("metric:{label}")
    } else {
        format!("metric:{label} ({group})")
    }
}

/// SuperGrok's overall meter arrives as "<Window> usage" (older builds: "<Window>
/// Build credits"); the card title already names the product, so the key keeps
/// only the window. Mirrors the popover's `metricLabel`.
fn metric_label(entry_id: &str, label: &str) -> String {
    let slug = entry_id
        .split('@')
        .next()
        .unwrap_or(entry_id)
        .to_ascii_lowercase();
    if slug != "supergrok" {
        return label.to_string();
    }
    let label = strip_trailing_word(label, "build credits");
    strip_trailing_word(&label, "usage")
}

/// `label` without a trailing `word` (ASCII case-insensitive) and the
/// whitespace before it; the JS `/\s+word$/i`. Needs at least one space, so a
/// label that merely ends in the letters is kept.
fn strip_trailing_word(label: &str, word: &str) -> String {
    if label.len() > word.len() && label.to_ascii_lowercase().ends_with(word) {
        let head = &label[..label.len() - word.len()];
        let trimmed = head.trim_end();
        if trimmed.len() < head.len() {
            return trimmed.to_string();
        }
    }
    label.to_string()
}

/// Layout of the compact Bars glyph in the given square, OpenUsage `MenuBarBars`.
#[derive(Debug, Clone, Copy)]
pub struct BarsLayout {
    pub n: usize,
    pub track_x: f64,
    pub track_w: f64,
    pub track_h: f64,
    pub rx: f64,
    pub y_offset: f64,
    pub gap: f64,
}

pub fn bars_layout(count: usize, side: f64) -> BarsLayout {
    let n = count.clamp(1, MAX_BARS);
    let pad = (side * 0.08).round().max(1.0);
    let gap = (side * 0.03).round().max(1.0);
    let track_x = pad;
    let track_w = side - 2.0 * pad;
    let layout_n = n.max(2) as f64;
    let track_h = ((side - 2.0 * pad - (layout_n - 1.0) * gap) / layout_n)
        .floor()
        .max(1.0);
    let rx = (track_h / 3.0).floor().max(1.0);
    let total_h = n as f64 * track_h + (n as f64 - 1.0) * gap;
    let y_offset = pad + ((side - 2.0 * pad - total_h) / 2.0).floor();
    BarsLayout {
        n,
        track_x,
        track_w,
        track_h,
        rx,
        y_offset,
        gap,
    }
}

/// Template RGBA (black ink, alpha as coverage) for the compact Bars glyph.
/// Empty fractions yield a fully transparent square.
pub fn bars_rgba(fractions: &[f64], side: u32) -> Vec<u8> {
    let side = side.max(8);
    let mut buf = vec![0u8; side as usize * side as usize * 4];
    if fractions.is_empty() {
        return buf;
    }
    let size = f64::from(side);
    let layout = bars_layout(fractions.len(), size);
    let n = layout.n;

    for (i, fraction) in fractions.iter().copied().take(n).enumerate() {
        let y = layout.y_offset + i as f64 * (layout.track_h + layout.gap) + 1.0;
        stamp_round_rect(
            &mut buf,
            side,
            RoundBar {
                x: layout.track_x,
                y,
                w: layout.track_w,
                h: layout.track_h,
                r_left: layout.rx,
                r_right: layout.rx,
            },
            41, // 0.16 * 255
        );
        let fill = bar_fill(layout.track_w, fraction);
        if fill.fill_w > 0.0 {
            let trailing = if fill.fill_w >= layout.track_w {
                layout.rx
            } else {
                (layout.rx * 0.35).floor().max(0.0)
            };
            stamp_round_rect(
                &mut buf,
                side,
                RoundBar {
                    x: layout.track_x,
                    y,
                    w: fill.fill_w,
                    h: layout.track_h,
                    r_left: layout.rx,
                    r_right: trailing,
                },
                255,
            );
        }
        if fill.remainder_w > 0.0
            && let Some(divider_x) = fill.divider_x
        {
            // A remainder that starts at 0 is the whole track of an empty
            // gauge: round both ends like the track, not like a tail meeting
            // a fill.
            let r_left = if divider_x <= 0.0 {
                layout.rx
            } else {
                (layout.rx * 0.2).floor().max(0.0)
            };
            stamp_round_rect(
                &mut buf,
                side,
                RoundBar {
                    x: layout.track_x + divider_x,
                    y,
                    w: fill.remainder_w,
                    h: layout.track_h,
                    r_left,
                    r_right: layout.rx,
                },
                61, // 0.24 * 255
            );
        }
    }
    buf
}

struct RoundBar {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    r_left: f64,
    r_right: f64,
}

fn stamp_round_rect(buf: &mut [u8], side: u32, bar: RoundBar, alpha: u8) {
    if bar.w <= 0.0 || bar.h <= 0.0 {
        return;
    }
    let side_i = side as i32;
    let min_x = bar.x.floor().max(0.0) as i32;
    let min_y = bar.y.floor().max(0.0) as i32;
    let max_x = (bar.x + bar.w).ceil().min(f64::from(side)) as i32;
    let max_y = (bar.y + bar.h).ceil().min(f64::from(side)) as i32;
    let target = f64::from(alpha);
    for py in min_y..max_y {
        for px in min_x..max_x {
            if px < 0 || py < 0 || px >= side_i || py >= side_i {
                continue;
            }
            let cover = sample_round_rect(f64::from(px) + 0.5, f64::from(py) + 0.5, &bar);
            if cover <= 0.0 {
                continue;
            }
            let idx = ((py as u32 * side + px as u32) * 4) as usize;
            let stamped = (target * cover).round().clamp(0.0, 255.0) as u8;
            if stamped > buf[idx + 3] {
                buf[idx] = 0;
                buf[idx + 1] = 0;
                buf[idx + 2] = 0;
                buf[idx + 3] = stamped;
            }
        }
    }
}

fn sample_round_rect(px: f64, py: f64, bar: &RoundBar) -> f64 {
    (0.5 - sd_round_box(px, py, bar)).clamp(0.0, 1.0)
}

fn sd_round_box(px: f64, py: f64, bar: &RoundBar) -> f64 {
    let r = if px < bar.x + bar.w * 0.5 {
        bar.r_left
    } else {
        bar.r_right
    };
    let r = r.max(0.0).min(bar.h * 0.5).min(bar.w * 0.5);
    let cx = bar.x + bar.w * 0.5;
    let cy = bar.y + bar.h * 0.5;
    let dx = (px - cx).abs() - (bar.w * 0.5 - r);
    let dy = (py - cy).abs() - (bar.h * 0.5 - r);
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    outside + dx.min(0.0).max(dy.min(0.0)) - r
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn zero_or_negative_fraction_draws_no_fill_but_a_visible_track() {
        let empty = bar_fill(100.0, 0.0);
        assert_eq!(empty.fill_w, 0.0);
        assert_eq!(empty.remainder_w, 100.0);
        assert_eq!(empty.divider_x, Some(0.0));
        assert_eq!(bar_fill(100.0, -0.5).fill_w, 0.0);
        // A non-finite fraction is the only fully invisible case.
        let nan = bar_fill(100.0, f64::NAN);
        assert_eq!(nan.remainder_w, 0.0);
        assert_eq!(nan.divider_x, None);
    }

    #[test]
    fn full_fraction_fills_track_with_no_remainder() {
        let fill = bar_fill(100.0, 1.0);
        assert_eq!(fill.fill_w, 100.0);
        assert_eq!(fill.remainder_w, 0.0);
        assert_eq!(fill.divider_x, None);
    }

    #[test]
    fn near_full_keeps_a_visible_tail() {
        let fill = bar_fill(100.0, 0.97);
        assert!(fill.fill_w < 100.0);
        assert!(fill.remainder_w >= 20.0);
        assert_eq!(fill.divider_x, Some(fill.fill_w));
    }

    #[test]
    fn visual_fraction_quantizes_near_full_in_fifteen_percent_steps() {
        assert!((visual_fraction(0.97) - 0.85).abs() < 0.0001);
        assert_eq!(visual_fraction(1.0), 1.0);
        assert_eq!(visual_fraction(0.0), 0.0);
        assert!((visual_fraction(0.5) - 0.5).abs() < 0.0001);
    }

    fn sample_payload() -> Value {
        json!({
            "entries": [
                {
                    "id": "anthropic",
                    "display_name": "Claude",
                    "status": "ready",
                    "sections": [
                        {"type": "metric", "label": "Weekly", "percent": 19, "value": "19%"},
                        {"type": "metric", "label": "Session", "percent": 41, "value": "41%"}
                    ]
                },
                {
                    "id": "openai",
                    "display_name": "Codex",
                    "status": "ready",
                    "sections": [
                        {"type": "metric", "label": "Codex weekly", "percent": 2, "value": "2%"}
                    ]
                },
                {
                    "id": "cursor",
                    "display_name": "Cursor",
                    "status": "error",
                    "sections": [
                        {"type": "metric", "label": "Cursor Models", "percent": 90, "value": "90%"}
                    ]
                }
            ]
        })
    }

    /// The popover stores stars under these keys; a label rule changed on one
    /// side only must fail here, not drop a starred bar from the menu bar.
    #[test]
    fn metric_keys_match_the_popover_fixture() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/strip_metric_keys.json"))
                .expect("fixture is JSON");
        let cases = fixture["cases"].as_array().expect("cases");
        assert!(!cases.is_empty());
        for case in cases {
            let id = case["id"].as_str().unwrap();
            let label = case["label"].as_str().unwrap();
            let group = case["group"].as_str().unwrap_or("");
            assert_eq!(
                metric_key(id, label, group),
                case["key"].as_str().unwrap(),
                "{case}"
            );
        }
    }

    /// SuperGrok's meter was renamed "Weekly usage" while the host kept
    /// stripping only "Build credits": a star saved as `metric:Weekly` stopped
    /// painting its bar.
    #[test]
    fn a_starred_supergrok_meter_is_painted() {
        let payload = serde_json::json!({ "entries": [
            { "id": "supergrok", "status": "ready", "sections": [
                { "type": "metric", "label": "Weekly usage", "percent": 3 }
            ]}
        ]});
        let mut stars = Stars::new();
        stars.insert("supergrok".into(), vec!["metric:Weekly".into()]);
        let content = content_from_payload(&payload, &stars, &["supergrok".into()]);
        assert_eq!(content.bars.len(), 1);
        assert_eq!(content.bars[0].key, "metric:Weekly");
    }

    #[test]
    fn empty_stars_fall_back_to_first_two_bounded_metrics() {
        let content = content_from_payload(&sample_payload(), &Stars::new(), &[]);
        assert_eq!(content.groups.len(), 2);
        assert_eq!(content.groups[0].2.len(), 2);
        assert_eq!(content.groups[0].2[0].key, "metric:Weekly");
        assert_eq!(content.groups[0].2[1].key, "metric:Session");
        assert_eq!(content.groups[1].2[0].key, "metric:Codex weekly");
        assert_eq!(content.bars.len(), 3);
        assert!(!content.title_line().is_empty());
    }

    #[test]
    fn stars_select_named_metrics_and_skip_errors() {
        let mut stars = Stars::new();
        stars.insert("anthropic".into(), vec!["metric:Session".into()]);
        stars.insert("cursor".into(), vec!["metric:Cursor Models".into()]);
        let content = content_from_payload(&sample_payload(), &stars, &[]);
        assert_eq!(content.groups.len(), 1);
        assert_eq!(content.groups[0].2[0].key, "metric:Session");
        assert_eq!(content.groups[0].2[0].fraction, 0.41);
        assert_eq!(content.bars.len(), 1);
    }

    #[test]
    fn grouped_metric_keys_match_the_popover() {
        let payload = json!({
            "entries": [{
                "id": "antigravity",
                "display_name": "Antigravity",
                "status": "ready",
                "sections": [
                    {"type": "text", "label": "Session", "value": ""},
                    {"type": "metric", "label": "Gemini", "percent": 4, "value": "4%"},
                    {"type": "text", "label": "Weekly", "value": ""},
                    {"type": "metric", "label": "Gemini", "percent": 11, "value": "11%"}
                ]
            }]
        });
        let content = content_from_payload(&payload, &Stars::new(), &[]);
        assert_eq!(content.groups[0].2[0].key, "metric:Gemini (Session)");
        assert_eq!(content.groups[0].2[1].key, "metric:Gemini (Weekly)");
    }

    /// A metric that names its own group (#213's report field — SuperGrok's
    /// slices, #255's Claude CLI sessions) keys and labels exactly as the same
    /// group arriving positionally would, so a star saved from the popover's
    /// grouped row still paints the menu-bar bar.
    #[test]
    fn metric_level_groups_key_like_positional_ones() {
        let payload = json!({
            "entries": [{
                "id": "anthropic",
                "display_name": "Claude",
                "status": "ready",
                "sections": [
                    {"type": "metric", "label": "Weekly (7d)", "percent": 32, "value": "32%"},
                    {"type": "metric", "label": "ship the release", "percent": 91,
                     "value": "91%", "group": "Sessions"},
                    {"type": "metric", "label": "Grok Build", "percent": 94,
                     "value": "94%", "group": "Breakdown"}
                ]
            }]
        });
        let mut stars = Stars::new();
        stars.insert(
            "anthropic".into(),
            vec![
                "metric:ship the release (Sessions)".into(),
                "metric:Grok Build (Breakdown)".into(),
            ],
        );
        let content = content_from_payload(&payload, &stars, &[]);
        let bars = &content.groups[0].2;
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].key, "metric:ship the release (Sessions)");
        assert_eq!(bars[0].label, "ship the release (Sessions)");
        assert_eq!(bars[1].key, "metric:Grok Build (Breakdown)");
        assert_eq!(bars[1].label, "Grok Build (Breakdown)");
    }

    /// The Left reading's text matches the popover's `leftPercent`
    /// (`max(0, 100 - percent)`); a value headline keeps its value.
    #[test]
    fn left_value_is_the_remaining_percent() {
        let payload = json!({"entries":[{"id":"zai", "status":"ready", "sections":[
            {"type":"metric", "label":"Weekly", "percent":18, "value":"18%"},
            {"type":"metric", "label":"Over", "percent":130, "value":"130%"},
            {"type":"metric", "label":"Credits", "percent":40, "value":"$4 of $10", "headline":"value"},
            {"type":"metric", "label":"Balance", "value":"$40"}]}]});
        let (_, _, metrics) = quota_group(&payload, "zai", &[]).unwrap();
        let left: Vec<Option<&str>> = metrics.iter().map(|m| m.left_value.as_deref()).collect();
        assert_eq!(left, vec![Some("82%"), Some("0%"), None, None]);
    }

    /// A metric switched off in the popover's Customize is not a quota window
    /// the Quattro look can pick; with the windows hidden a grouped row stands
    /// in, and with everything hidden the entry has nothing to show.
    #[test]
    fn quota_group_leaves_out_hidden_metrics() {
        let payload = json!({"entries":[{"id":"zai", "status":"ready", "sections":[
            {"type":"metric", "label":"Session (5h)", "percent":0, "value":"0%"},
            {"type":"metric", "label":"MCP tools (monthly)", "percent":18, "value":"18%"},
            {"type":"metric", "label":"Glm", "group":"Models", "percent":40, "value":"40%"}]}]});
        let keys = |hidden: &[&str]| {
            let hidden: Vec<String> = hidden.iter().map(|k| k.to_string()).collect();
            quota_group(&payload, "zai", &hidden)
                .map(|(_, _, metrics)| metrics.into_iter().map(|m| m.key).collect::<Vec<_>>())
        };

        assert_eq!(
            keys(&[]),
            Some(vec![
                "metric:Session (5h)".to_string(),
                "metric:MCP tools (monthly)".to_string()
            ])
        );
        assert_eq!(
            keys(&["metric:MCP tools (monthly)"]),
            Some(vec!["metric:Session (5h)".to_string()])
        );
        assert_eq!(
            keys(&["metric:Session (5h)", "metric:MCP tools (monthly)"]),
            Some(vec!["metric:Glm (Models)".to_string()])
        );
        assert_eq!(
            keys(&[
                "metric:Session (5h)",
                "metric:MCP tools (monthly)",
                "metric:Glm (Models)"
            ]),
            None
        );
    }

    /// `hidden_rows` is optional: an older popover, or a malformed field,
    /// hides nothing; blank ids and keys and repeats are dropped.
    #[test]
    fn parse_hidden_rows_reads_the_strip_ipc() {
        let hidden = parse_hidden_rows(&json!({"hidden_rows": {
            "zai": [" metric:MCP tools (monthly) ", "metric:MCP tools (monthly)", "", 7],
            "": ["metric:Weekly"],
            "openai": [],
            "kimi": "metric:Weekly"
        }}));
        assert_eq!(hidden.len(), 1);
        assert_eq!(
            hidden["zai"],
            vec!["metric:MCP tools (monthly)".to_string()]
        );
        assert!(parse_hidden_rows(&json!({"stars": {}})).is_empty());
        assert!(parse_hidden_rows(&json!({"hidden_rows": ["zai"]})).is_empty());
    }

    #[test]
    fn parse_strip_ipc_caps_stars_at_two_per_provider() {
        let value = json!({
            "style": "text",
            "stars": {
                "anthropic": ["metric:Weekly", "metric:Session", "metric:Extra"],
                "": ["x"],
                "openai": ["metric:Codex weekly"]
            }
        });
        let (style, stars, order) = parse_strip_ipc(&value);
        assert_eq!(style, StripStyle::Bars);
        assert_eq!(stars["anthropic"].len(), 2);
        assert_eq!(stars["openai"], vec!["metric:Codex weekly".to_string()]);
        assert!(!stars.contains_key(""));
        assert!(order.is_empty());
    }

    #[test]
    fn strip_follows_popover_card_order() {
        let mut stars = Stars::new();
        stars.insert("anthropic".into(), vec!["metric:Weekly".into()]);
        stars.insert("openai".into(), vec!["metric:Codex weekly".into()]);
        let content = content_from_payload(
            &sample_payload(),
            &stars,
            &["openai".into(), "anthropic".into()],
        );
        assert_eq!(content.groups[0].0, "openai");
        assert_eq!(content.groups[1].0, "anthropic");
        assert_eq!(content.bars[0].provider_id, "openai");
        assert_eq!(content.bars[1].provider_id, "anthropic");
    }

    #[test]
    fn bars_rgba_is_square_black_and_anti_aliased() {
        let bytes = bars_rgba(&[0.4, 0.97], BARS_PIXEL_SIDE);
        assert_eq!(
            bytes.len(),
            (BARS_PIXEL_SIDE * BARS_PIXEL_SIDE * 4) as usize
        );
        assert_eq!(bytes[3], 0, "top-left stays transparent");
        let last = bytes.len() - 1;
        assert_eq!(bytes[last], 0, "bottom-right stays transparent");
        let mut inked = 0usize;
        let mut soft = 0usize;
        for pixel in bytes.as_chunks::<4>().0 {
            if pixel[3] > 0 {
                inked += 1;
                assert_eq!(&pixel[..3], &[0, 0, 0]);
            }
            if pixel[3] > 0 && pixel[3] < 255 {
                soft += 1;
            }
        }
        assert!(inked > 40, "glyph too sparse: {inked}");
        assert!(soft > 0, "expected anti-aliased edges");
    }

    #[test]
    fn empty_fractions_are_fully_transparent() {
        let bytes = bars_rgba(&[], BARS_PIXEL_SIDE);
        assert!(bytes.as_chunks::<4>().0.iter().all(|p| p[3] == 0));
    }

    /// A 0% row keeps a visible capsule: before, only its faint 0.16-alpha
    /// track was stamped, which vanished against the menu bar — the row read
    /// as a missing bar instead of an empty gauge, the popover's meter being
    /// plainly visible at the same 0%.
    #[test]
    fn empty_bars_keep_a_visible_capsule() {
        let bytes = bars_rgba(&[0.0, 0.5], BARS_PIXEL_SIDE);
        let side = BARS_PIXEL_SIDE as usize;
        let layout = bars_layout(2, f64::from(BARS_PIXEL_SIDE));
        let y = (layout.y_offset + 1.0 + layout.track_h / 2.0) as usize;
        let mut peak = 0u8;
        let mut visible_cols = 0usize;
        for x in 0..side {
            let alpha = bytes[(y * side + x) * 4 + 3];
            peak = peak.max(alpha);
            if alpha >= 61 {
                visible_cols += 1;
            }
        }
        assert_eq!(peak, 61, "empty row peaks at the remainder alpha");
        assert!(
            visible_cols > side / 2,
            "capsule spans the track: {visible_cols}"
        );
    }

    #[test]
    fn bars_rgba_at_each_status_item_scale() {
        for scale in BARS_SCALES {
            let side = BARS_POINT_SIDE * scale;
            let bytes = bars_rgba(&[0.4, 0.97], side);
            assert_eq!(bytes.len(), (side * side * 4) as usize, "scale {scale}");
            assert!(
                bytes.as_chunks::<4>().0.iter().any(|p| p[3] > 0),
                "scale {scale} produced an empty glyph"
            );
        }
    }
}
