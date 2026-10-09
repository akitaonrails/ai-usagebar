//! Chart, logo or short-name strip for the macOS status item.
//!
//! `StripContent` owns visible groups and their values; the report supplies
//! the short name drawn in place of a mark, either because the provider has
//! no embedded mark or because the user chose the Quattro look.

use serde_json::Value;

use super::strip::{HiddenRows, StripContent, StripMetric, quota_group};

/// Artwork needed for the current status-item content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StatusItemContent {
    /// The static application icon is the empty-content fallback.
    AppIcon,
    /// Draw the usage bars.
    Chart,
    /// Draw provider logos and one summary value each.
    Logos,
}

/// The menu-bar look chosen in Settings → Menu Bar → Menu Bar Shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MenuBarLook {
    /// The usage bars (default).
    Chart,
    /// Each starred provider's logo followed by its highest quota usage.
    Logos,
    /// One selected provider's logo, short name (`cld`, `cdx`, …) and highest
    /// quota usage, the way the Quattro bar tags a provider.
    Quattro,
}

impl MenuBarLook {
    /// Read `[tray] menu_bar_style`. Anything unrecognised keeps the chart, so
    /// a config written by a newer build never blanks the status item. `name`
    /// is what 1.32.0 called the Quattro look and is still read as it.
    pub(super) fn from_style(style: Option<&str>) -> Self {
        match style {
            Some("provider") => Self::Logos,
            Some("quattro" | "name") => Self::Quattro,
            _ => Self::Chart,
        }
    }

    /// The `[tray] menu_bar_style` value persisted for this look.
    pub(super) fn style(self) -> &'static str {
        match self {
            Self::Chart => "bars",
            Self::Logos => "provider",
            Self::Quattro => "quattro",
        }
    }

    /// The value the popover's picker uses for this look.
    pub(super) fn picker_value(self) -> &'static str {
        match self {
            Self::Chart => "chart",
            Self::Logos => "logos",
            Self::Quattro => "quattro",
        }
    }

    /// Parse the popover's picker value; `name` is the 1.32.0 spelling.
    pub(super) fn from_picker_value(value: &str) -> Option<Self> {
        match value {
            "chart" => Some(Self::Chart),
            "logos" => Some(Self::Logos),
            "quattro" | "name" => Some(Self::Quattro),
            _ => None,
        }
    }
}

/// Select status-item artwork, falling back to the app icon when content is empty.
pub(super) fn status_item_content(look: MenuBarLook, has_content: bool) -> StatusItemContent {
    if !has_content {
        StatusItemContent::AppIcon
    } else if look == MenuBarLook::Chart {
        StatusItemContent::Chart
    } else {
        StatusItemContent::Logos
    }
}

/// How the popover reads a quota (Preferences → Show Usage As). The menu
/// bar's percentages follow it, so the chip says `82%` left when the tab
/// that selects it does, not the `18%` used behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UsageReading {
    Used,
    Left,
}

impl UsageReading {
    /// Read the `strip` IPC's `show_as`. Anything else keeps used, the
    /// reading the menu bar had before the popover reports one.
    pub(super) fn from_strip_ipc(value: &Value) -> Self {
        match value.get("show_as").and_then(Value::as_str) {
            Some("left") => Self::Left,
            _ => Self::Used,
        }
    }
}

/// A metric's text in `reading`: its remaining share in Left when it has a
/// percent, its report value otherwise.
fn metric_text(metric: &StripMetric, reading: UsageReading) -> &str {
    match (reading, metric.left_value.as_deref()) {
        (UsageReading::Left, Some(left)) => left,
        _ => metric.value.trim(),
    }
}

/// The fractions painted by the compact Bars glyph (StatusItemContent::Chart):
/// always the consumed share, whatever the popover's `show_as` reading. Ink is
/// consumption — the monochrome glyph has no color to anchor a flipped fill,
/// so when #389 made it fill with what remains, the healthiest windows painted
/// the most ink while a spent one shrank to a barely visible track, telling
/// the popover's story backwards. The reading stays what its name says — how
/// the chip text reads — and never the bar geometry.
pub(super) fn chart_fractions(content: &StripContent) -> Vec<f64> {
    content.bars.iter().map(|metric| metric.fraction).collect()
}

/// One entry of the emergency menu attached to the status item when the
/// popover's WKWebView could not be built (#249).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FallbackItem {
    /// muda menu id the host matches `MenuEvent`s against.
    pub(super) id: &'static str,
    pub(super) label: &'static str,
}

/// The fallback status-item menu for the webview-less tray (#249): an
/// accessory app (no Dock icon, no app menu) otherwise has no quit affordance
/// short of `killall`. Refresh stays because the status-item readout works
/// without the webview; Quit terminates through the same loop exit the
/// popover's own Quit control uses.
pub(super) fn fallback_menu_items() -> [FallbackItem; 2] {
    [
        FallbackItem {
            id: "fallback-refresh",
            label: "Refresh",
        },
        FallbackItem {
            id: "fallback-quit",
            label: "Quit AI Usage",
        },
    ]
}

/// The fallback menu exists **only** while the popover's webview is absent.
/// Normal operation keeps the status item menu-free (see `build_tray`) so
/// both mouse buttons reach the popover; attaching a menu makes AppKit
/// intercept clicks, which is exactly the trade wanted when there is no
/// popover to open.
pub(super) fn fallback_menu_attached(webview_built: bool) -> bool {
    !webview_built
}

/// One provider's visible logo (or short name) and summary value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LogoSegment {
    /// Lowercase provider slug used to find its embedded mark.
    pub(super) slug: String,
    /// Report short name, drawn when the mark cannot be drawn, or beside it
    /// when `with_name` is set.
    pub(super) short_name: Option<String>,
    /// The Quattro look: draw `short_name` after the mark rather than only
    /// where the mark is missing.
    pub(super) with_name: bool,
    /// One non-empty summary value, displayed on a single line.
    pub(super) values: Vec<String>,
}

/// Build logo segments from the same starred metric groups used by the chart.
/// The Quattro look keeps one provider, `selected` or its fallbacks, and its
/// highest window, like the Quattro bar's `icon SHORT 54%` chip, with the short name
/// beside the mark unless `show_short_name` is off. Logos keeps every starred
/// provider, with one value for its highest visible quota usage. `hidden`
/// holds metrics switched off in Customize, which both summaries leave out.
pub(super) fn logo_segments(
    content: &StripContent,
    report: &Value,
    look: MenuBarLook,
    selected: Option<&str>,
    show_short_name: bool,
    reading: UsageReading,
    hidden: &HiddenRows,
) -> Vec<LogoSegment> {
    if look == MenuBarLook::Quattro {
        return name_segment(content, report, selected, show_short_name, reading, hidden)
            .into_iter()
            .collect();
    }
    content
        .groups
        .iter()
        .filter_map(|(id, _, _)| {
            let hidden_keys = hidden.get(id).map(Vec::as_slice).unwrap_or_default();
            let (_, _, metrics) = quota_group(report, id, hidden_keys)?;
            quota_chip(id, &metrics, report, false, reading)
        })
        .collect()
}

/// The Quattro look's one chip, for the popover's selected provider, else the
/// report's `primary`, else the first starred group, skipping any without a
/// value. Its value comes from every quota window of that provider, stars
/// aside and hidden metrics left out, like the Quattro bar and the popover
/// tab that selects it. A provider whose every metric is hidden has no value
/// and is skipped the same way.
fn name_segment(
    content: &StripContent,
    report: &Value,
    selected: Option<&str>,
    show_short_name: bool,
    reading: UsageReading,
    hidden: &HiddenRows,
) -> Option<LogoSegment> {
    let primary = report.get("primary").and_then(Value::as_str);
    let starred = content.groups.iter().map(|(id, _, _)| id.as_str());
    [selected, primary]
        .into_iter()
        .flatten()
        .chain(starred)
        .find_map(|id| {
            let hidden_keys = hidden.get(id).map(Vec::as_slice).unwrap_or_default();
            let (id, _, metrics) = quota_group(report, id, hidden_keys)?;
            quota_chip(&id, &metrics, report, show_short_name, reading)
        })
}

/// The chip shows the provider's highest-percent metric, like the Quattro
/// bar's default `auto` window (`omarchy/Model.js` `maxPercent`): a spent
/// weekly window must not hide behind an idle 5h session reading 0%.
/// Without `show_short_name` the mark stands alone, like the logos look,
/// which still falls back to the name for a provider with no mark.
fn quota_chip(
    id: &str,
    metrics: &[StripMetric],
    report: &Value,
    show_short_name: bool,
    reading: UsageReading,
) -> Option<LogoSegment> {
    let highest = highest_metric(metrics)?;
    segment(
        id,
        std::slice::from_ref(highest),
        report,
        1,
        show_short_name,
        reading,
    )
}

/// The bounded metric with the largest used fraction, the first one on a
/// tie; the first metric with a value when none is bounded. A value headline
/// (a prepaid balance) is a figure, not a quota window, so it never wins the
/// race — the same rule the popover tab applies (`previewMetric` ranks only
/// percent headlines) — and stands in through the fallback when the provider
/// has nothing else.
fn highest_metric(metrics: &[StripMetric]) -> Option<&StripMetric> {
    let shown = || metrics.iter().filter(|m| !m.value.trim().is_empty());
    let highest_bounded = shown().filter(|m| m.bounded && !m.value_headline).fold(
        None,
        |best: Option<&StripMetric>, m| match best {
            Some(best) if best.fraction >= m.fraction => Some(best),
            _ => Some(m),
        },
    );
    highest_bounded.or_else(|| shown().next())
}

/// One provider's segment: up to `value_cap` non-empty values in `reading`,
/// or `None` when it has no value or no usable slug.
fn segment(
    id: &str,
    metrics: &[StripMetric],
    report: &Value,
    value_cap: usize,
    with_name: bool,
    reading: UsageReading,
) -> Option<LogoSegment> {
    let values: Vec<String> = metrics
        .iter()
        .map(|metric| metric_text(metric, reading))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .take(value_cap)
        .collect();
    if values.is_empty() {
        return None;
    }
    let slug = id.split('@').next()?.to_ascii_lowercase();
    if slug.is_empty() {
        return None;
    }
    Some(LogoSegment {
        slug,
        short_name: short_name_for(report, id),
        with_name,
        values,
    })
}

/// Build tooltip lines for the starred groups in `reading`, or the app name
/// when none have values.
pub(super) fn tooltip(content: &StripContent, reading: UsageReading) -> String {
    let lines: Vec<String> = content
        .groups
        .iter()
        .filter_map(|(_, name, metrics)| {
            let values: Vec<&str> = metrics
                .iter()
                .map(|metric| metric_text(metric, reading))
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
    fn logos_show_one_highest_quota_instead_of_reset_shorter_windows() {
        let report = json!({"entries": [
            {"id":"anthropic", "status":"ready", "sections":[
                {"type":"metric", "label":"Session (5h)", "percent":2, "window_secs":18000},
                {"type":"metric", "label":"Weekly (7d)", "percent":25, "window_secs":604800},
                {"type":"metric", "label":"Fable (7d)", "percent":12, "window_secs":604800}
            ]},
            {"id":"openai@work", "status":"ready", "sections":[
                {"type":"metric", "label":"Session", "percent":0, "window_secs":18000},
                {"type":"metric", "label":"Weekly", "percent":0, "window_secs":604800},
                {"type":"metric", "label":"Monthly", "percent":75, "window_secs":2592000},
                {"type":"metric", "label":"Context", "group":"Sessions", "percent":99}
            ]}
        ]});
        // Stars select which providers appear, not the allowance to summarize.
        let content = super::super::strip::content_from_payload(
            &report,
            &super::super::strip::Stars::new(),
            &["openai@work".into(), "anthropic".into()],
        );
        let read = |reading| {
            logo_segments(
                &content,
                &report,
                MenuBarLook::Logos,
                None,
                false,
                reading,
                &HiddenRows::new(),
            )
        };
        let left = read(UsageReading::Left);
        assert_eq!(left.len(), 2);
        assert_eq!(left[0].slug, "openai");
        assert_eq!(left[0].values, ["25%"]);
        assert_eq!(left[1].values, ["75%"]);
        let used = read(UsageReading::Used);
        assert_eq!(used[0].values, ["75%"]);
        assert_eq!(used[1].values, ["25%"]);
    }

    #[test]
    fn logos_match_the_name_chip_for_short_model_and_hidden_windows() {
        let report = json!({"entries":[
            entry("anthropic", "cld", &[("Session", 90.0), ("Weekly", 25.0), ("Fable", 99.0)])
        ]});
        let content = starred(&["anthropic"]);
        for (hidden_keys, expected) in [
            (vec![], "1%"),
            (vec!["metric:Fable"], "10%"),
            (vec!["metric:Fable", "metric:Session"], "75%"),
        ] {
            let hidden = HiddenRows::from([(
                "anthropic".into(),
                hidden_keys.into_iter().map(String::from).collect(),
            )]);
            for look in [MenuBarLook::Logos, MenuBarLook::Quattro] {
                let segments = logo_segments(
                    &content,
                    &report,
                    look,
                    Some("anthropic"),
                    false,
                    UsageReading::Left,
                    &hidden,
                );
                assert_eq!(segments[0].values, [expected], "{look:?}");
            }
        }
    }

    #[test]
    fn empty_modes_fall_back_to_static_app_icon() {
        for look in [MenuBarLook::Chart, MenuBarLook::Logos, MenuBarLook::Quattro] {
            assert_eq!(
                status_item_content(look, false),
                StatusItemContent::AppIcon,
                "{look:?}"
            );
        }
        assert_eq!(
            status_item_content(MenuBarLook::Chart, true),
            StatusItemContent::Chart
        );
        assert_eq!(
            status_item_content(MenuBarLook::Logos, true),
            StatusItemContent::Logos
        );
        // Quattro reuses the strip image: only the label differs from Logos.
        assert_eq!(
            status_item_content(MenuBarLook::Quattro, true),
            StatusItemContent::Logos
        );
    }

    #[test]
    fn menu_bar_look_round_trips_through_config_and_picker() {
        for look in [MenuBarLook::Chart, MenuBarLook::Logos, MenuBarLook::Quattro] {
            assert_eq!(MenuBarLook::from_style(Some(look.style())), look);
            assert_eq!(
                MenuBarLook::from_picker_value(look.picker_value()),
                Some(look)
            );
        }
    }

    /// An unset, unknown or future `menu_bar_style` keeps the chart default.
    #[test]
    fn menu_bar_look_defaults_to_the_chart() {
        assert_eq!(MenuBarLook::from_style(None), MenuBarLook::Chart);
        assert_eq!(MenuBarLook::from_style(Some("bars")), MenuBarLook::Chart);
        assert_eq!(
            MenuBarLook::from_style(Some("sparkles")),
            MenuBarLook::Chart
        );
        assert_eq!(MenuBarLook::from_picker_value("sparkles"), None);
    }

    /// 1.32.0 persisted this look as `name`; a config or a popover still
    /// saying so keeps the look instead of falling back to the chart.
    #[test]
    fn the_1_32_0_name_spelling_still_selects_the_quattro_look() {
        assert_eq!(MenuBarLook::from_style(Some("name")), MenuBarLook::Quattro);
        assert_eq!(
            MenuBarLook::from_picker_value("name"),
            Some(MenuBarLook::Quattro)
        );
        assert_eq!(MenuBarLook::Quattro.style(), "quattro");
        assert_eq!(MenuBarLook::Quattro.picker_value(), "quattro");
    }

    /// #249: the emergency menu attaches only when the webview is absent, so
    /// normal operation — menu-free status item, clicks open the popover — is
    /// untouched.
    #[test]
    fn fallback_menu_attaches_only_without_the_webview() {
        assert!(!fallback_menu_attached(true));
        assert!(fallback_menu_attached(false));
    }

    /// The host matches `MenuEvent` ids against these strings, so they must be
    /// unique and Quit must be the terminal action of the menu.
    #[test]
    fn fallback_menu_items_have_unique_nonempty_ids_ending_in_quit() {
        let items = fallback_menu_items();
        assert!(items.iter().all(|item| !item.id.is_empty()));
        assert!(items.iter().all(|item| !item.label.is_empty()));
        let ids: Vec<&str> = items.iter().map(|item| item.id).collect();
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(ids.len(), unique.len(), "ids must be unique: {ids:?}");
        assert_eq!(items[items.len() - 1].id, "fallback-quit");
        assert_eq!(items[0].id, "fallback-refresh");
    }

    #[test]
    fn logo_segments_show_one_value_in_provider_order() {
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
        let report = json!({"entries":[
            entry("anthropic", "cld", &[("Weekly", 41.0)]),
            entry("openai", "gpt", &[("Weekly", 9.0), ("Reserve", 5.0)])
        ]});

        let segments = logo_segments(
            &content,
            &report,
            MenuBarLook::Logos,
            None,
            true,
            UsageReading::Used,
            &HiddenRows::new(),
        );
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].slug, "anthropic");
        assert_eq!(segments[0].values, vec![String::from("41%")]);
        assert_eq!(segments[1].slug, "openai");
        assert_eq!(segments[1].values, vec![String::from("9%")]);
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

        let segments = logo_segments(
            &content,
            &json!({"entries":[
                {"id":"cursor", "status":"ready", "sections":[]},
                entry("zai", "zai", &[("Weekly", 12.0)])
            ]}),
            MenuBarLook::Logos,
            None,
            true,
            UsageReading::Used,
            &HiddenRows::new(),
        );
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
        assert!(
            logo_segments(
                &content,
                &report,
                MenuBarLook::Logos,
                None,
                true,
                UsageReading::Used,
                &HiddenRows::new()
            )
            .is_empty()
        );
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
            entry("unknown@work", "unk", &[("Allowance", 8.0)])
        ]});

        let segments = logo_segments(
            &content,
            &report,
            MenuBarLook::Logos,
            None,
            true,
            UsageReading::Used,
            &HiddenRows::new(),
        );
        assert_eq!(segments[0].slug, "unknown");
        assert!(super::super::marks::mark_svg(&segments[0].slug).is_none());
        assert_eq!(segments[0].short_name.as_deref(), Some("unk"));
    }

    /// A ready report entry with one metric section per `(label, percent)`.
    fn entry(id: &str, short_name: &str, windows: &[(&str, f64)]) -> Value {
        let sections: Vec<Value> = windows
            .iter()
            .map(|(label, percent)| {
                json!({"type":"metric", "label":label, "percent":percent,
                       "value":format!("{percent}%")})
            })
            .collect();
        json!({"id":id, "short_name":short_name, "status":"ready", "sections":sections})
    }

    /// Strip content where each provider has one starred metric, in order.
    fn starred(ids: &[&str]) -> StripContent {
        StripContent {
            groups: ids
                .iter()
                .map(|id| (id.to_string(), id.to_string(), vec![metric("1%")]))
                .collect(),
            bars: Vec::new(),
        }
    }

    /// The Quattro look keeps the mark and adds the short name beside it; the
    /// logos look keeps the mark alone.
    #[test]
    fn name_look_adds_the_short_name_beside_the_mark() {
        let content = starred(&["anthropic"]);
        let report = json!({"entries":[entry("anthropic", "cld", &[("Session", 41.0)])]});

        let chip = logo_segments(
            &content,
            &report,
            MenuBarLook::Quattro,
            None,
            true,
            UsageReading::Used,
            &HiddenRows::new(),
        );
        assert!(chip[0].with_name);
        assert_eq!(chip[0].short_name.as_deref(), Some("cld"));
        assert!(super::super::marks::mark_svg(&chip[0].slug).is_some());

        let logos = logo_segments(
            &content,
            &report,
            MenuBarLook::Logos,
            None,
            true,
            UsageReading::Used,
            &HiddenRows::new(),
        );
        assert!(!logos[0].with_name);
    }

    /// With the short name turned off the chip is the mark and the value; a
    /// provider with no mark still gets its name, since nothing else would
    /// tell which provider the value belongs to.
    #[test]
    fn name_look_can_leave_out_the_short_name() {
        let content = starred(&["kimi", "unknown"]);
        let report = json!({"primary":null,"entries":[
            entry("kimi", "kmi", &[("Weekly", 42.0)]),
            entry("unknown", "unk", &[("Usage", 8.0)]),
        ]});

        let chip = logo_segments(
            &content,
            &report,
            MenuBarLook::Quattro,
            Some("kimi"),
            false,
            UsageReading::Used,
            &HiddenRows::new(),
        );
        assert!(!chip[0].with_name);
        assert!(super::super::marks::mark_svg(&chip[0].slug).is_some());
        assert_eq!(chip[0].values, vec![String::from("42%")]);

        let shown = logo_segments(
            &content,
            &report,
            MenuBarLook::Quattro,
            Some("kimi"),
            true,
            UsageReading::Used,
            &HiddenRows::new(),
        );
        assert!(shown[0].with_name);

        let unmarked = logo_segments(
            &content,
            &report,
            MenuBarLook::Quattro,
            Some("unknown"),
            false,
            UsageReading::Used,
            &HiddenRows::new(),
        );
        assert!(super::super::marks::mark_svg(&unmarked[0].slug).is_none());
        assert_eq!(unmarked[0].short_name.as_deref(), Some("unk"));
    }

    fn two_groups() -> StripContent {
        StripContent {
            groups: vec![
                (
                    "anthropic".into(),
                    "Claude".into(),
                    vec![metric("54%"), metric("16%")],
                ),
                ("openai".into(), "Codex".into(), vec![metric("100%")]),
            ],
            bars: Vec::new(),
        }
    }

    fn two_entries(primary: Option<&str>) -> Value {
        json!({"primary":primary, "entries":[
            entry("anthropic", "cld", &[("Session", 54.0), ("Weekly", 16.0)]),
            entry("openai", "gpt", &[("Session", 100.0)]),
        ]})
    }

    /// Quattro draws one chip for the selected provider: one provider and one
    /// value, where the logos look shows one value for every starred provider.
    #[test]
    fn name_look_shows_only_the_selected_provider_and_one_value() {
        let report = two_entries(Some("anthropic"));
        let chip = |selected| {
            logo_segments(
                &two_groups(),
                &report,
                MenuBarLook::Quattro,
                selected,
                true,
                UsageReading::Used,
                &HiddenRows::new(),
            )
        };

        let codex = chip(Some("openai"));
        assert_eq!(codex.len(), 1);
        assert_eq!(codex[0].slug, "openai");
        assert_eq!(codex[0].values, vec![String::from("100%")]);
        assert_eq!(chip(Some("anthropic"))[0].values, vec![String::from("54%")]);

        let logos = logo_segments(
            &two_groups(),
            &report,
            MenuBarLook::Logos,
            Some("openai"),
            true,
            UsageReading::Used,
            &HiddenRows::new(),
        );
        assert_eq!(logos.len(), 2);
        assert_eq!(logos[0].values.len(), 1);
    }

    /// Before the popover reports a selection the report's `primary` stands
    /// in, then the first starred provider; a stale selection (the provider
    /// was disabled) falls through the same way.
    #[test]
    fn name_look_falls_back_from_selection_to_primary_to_first() {
        let slug = |report: &Value, selected: Option<&str>| {
            logo_segments(
                &two_groups(),
                report,
                MenuBarLook::Quattro,
                selected,
                true,
                UsageReading::Used,
                &HiddenRows::new(),
            )[0]
            .slug
            .clone()
        };

        assert_eq!(slug(&two_entries(Some("openai")), None), "openai");
        assert_eq!(slug(&two_entries(Some("openai")), Some("gone")), "openai");
        assert_eq!(slug(&two_entries(None), None), "anthropic");
        assert_eq!(slug(&two_entries(None), Some("gone")), "anthropic");
    }

    /// Stars do not decide the Quattro look: a selected provider with nothing
    /// starred draws its own chip, never the primary's.
    #[test]
    fn name_look_shows_a_selected_provider_that_has_no_star() {
        let report = json!({"primary":"zai","entries":[
            entry("zai", "zai", &[("Session", 0.0)]),
            entry("supergrok", "sgk", &[("Weekly usage", 7.0)]),
        ]});
        let content = starred(&["zai"]);

        let chip = logo_segments(
            &content,
            &report,
            MenuBarLook::Quattro,
            Some("supergrok"),
            true,
            UsageReading::Used,
            &HiddenRows::new(),
        );
        assert_eq!(chip.len(), 1);
        assert_eq!(chip[0].slug, "supergrok");
        assert_eq!(chip[0].short_name.as_deref(), Some("sgk"));
        assert_eq!(chip[0].values, vec![String::from("7%")]);

        let unselected = logo_segments(
            &content,
            &report,
            MenuBarLook::Quattro,
            None,
            true,
            UsageReading::Used,
            &HiddenRows::new(),
        );
        assert_eq!(unselected[0].slug, "zai");
    }

    /// A provider with no value, or one whose fetch failed, is not a
    /// candidate, so the Quattro look never draws a bare name with no number.
    #[test]
    fn name_look_skips_a_selected_provider_without_a_value() {
        let report = json!({"primary":null,"entries":[
            {"id":"cursor", "status":"ready", "sections":[{"type":"metric", "label":"Usage"}]},
            {"id":"kimi", "status":"error", "sections":[
                {"type":"metric", "label":"Weekly", "percent":40, "value":"40%"}]},
            entry("zai", "zai", &[("Session", 12.0)]),
        ]});
        let content = starred(&["cursor", "kimi", "zai"]);

        for selected in ["cursor", "kimi"] {
            let segments = logo_segments(
                &content,
                &report,
                MenuBarLook::Quattro,
                Some(selected),
                true,
                UsageReading::Used,
                &HiddenRows::new(),
            );
            assert_eq!(segments.len(), 1);
            assert_eq!(segments[0].slug, "zai");
        }
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

        assert_eq!(
            tooltip(&content, UsageReading::Used),
            "Claude · 41% 5%\nCodex · 9%"
        );
        assert_eq!(
            tooltip(
                &StripContent {
                    groups: Vec::new(),
                    bars: Vec::new(),
                },
                UsageReading::Used
            ),
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
            left_value: None,
            value_headline: false,
            grouped: false,
        }
    }

    /// Like Quattro's `auto` window the chip shows the highest percent among
    /// every quota window, stars aside: a spent weekly window reads `100%`
    /// over an idle 5h session, and Z.AI's third, monthly MCP window at 18%
    /// reads `18%` over two at 0%.
    #[test]
    fn name_look_shows_the_highest_quota_window() {
        let chip = |zai: Value| {
            let report = json!({"primary":null, "entries":[zai]});
            logo_segments(
                &starred(&["zai"]),
                &report,
                MenuBarLook::Quattro,
                Some("zai"),
                true,
                UsageReading::Used,
                &HiddenRows::new(),
            )[0]
            .values
            .clone()
        };
        let spent_weekly = entry("zai", "zai", &[("Session (5h)", 0.0), ("Weekly", 100.0)]);
        assert_eq!(chip(spent_weekly), vec![String::from("100%")]);
        let monthly_third = entry(
            "zai",
            "zai",
            &[
                ("Session (5h)", 0.0),
                ("Weekly", 0.0),
                ("MCP tools (monthly)", 18.0),
            ],
        );
        assert_eq!(chip(monthly_third), vec![String::from("18%")]);
        let balance_first = json!({"id":"zai", "status":"ready", "sections":[
            {"type":"metric", "label":"Balance", "value":"$40"},
            {"type":"metric", "label":"Weekly", "percent":12, "value":"12%"}]});
        assert_eq!(chip(balance_first), vec![String::from("12%")]);
    }

    /// A grouped row (Claude's CLI sessions, SuperGrok's product slices) is a
    /// breakdown, not a window: it never outranks one, like Quattro's
    /// `selectMetric`, and stands in only when the provider has nothing else.
    #[test]
    fn name_look_ranks_grouped_rows_only_without_a_window() {
        let report = json!({"primary":null, "entries":[
            {"id":"anthropic", "status":"ready", "sections":[
                {"type":"metric", "label":"Session", "percent":0, "value":"0%"},
                {"type":"metric", "label":"cli-1", "group":"Sessions", "percent":90, "value":"90%"}]},
            {"id":"supergrok", "status":"ready", "sections":[
                {"type":"metric", "label":"Grok", "group":"Products", "percent":7, "value":"7%"}]},
        ]});
        let content = starred(&["anthropic", "supergrok"]);
        let value = |selected| {
            logo_segments(
                &content,
                &report,
                MenuBarLook::Quattro,
                Some(selected),
                true,
                UsageReading::Used,
                &HiddenRows::new(),
            )[0]
            .values
            .clone()
        };

        assert_eq!(value("anthropic"), vec![String::from("0%")]);
        assert_eq!(value("supergrok"), vec![String::from("7%")]);
    }

    /// A value headline that carries a percent (a prepaid balance meter, a
    /// Cursor credit grant) is a figure, not a quota window: it never
    /// outranks one — the popover tab the chip selects ranks only percent
    /// headlines (`previewMetric`), and on `main` the two could disagree.
    /// It stands in only when the provider has nothing else.
    #[test]
    fn name_look_ranks_value_headlines_only_without_a_window() {
        let report = json!({"primary":null, "entries":[
            {"id":"deepseek", "status":"ready", "sections":[
                {"type":"metric", "label":"Balance", "percent":95, "value":"$0.25", "headline":"value"},
                {"type":"metric", "label":"Weekly", "percent":12, "value":"12%"}]},
            {"id":"openrouter", "status":"ready", "sections":[
                {"type":"metric", "label":"Credit balance", "percent":40, "value":"$4 of $10", "headline":"value"}]},
        ]});
        let content = super::super::strip::content_from_payload(
            &report,
            &super::super::strip::Stars::new(),
            &[],
        );
        let chip = |selected, reading| {
            logo_segments(
                &content,
                &report,
                MenuBarLook::Quattro,
                Some(selected),
                true,
                reading,
                &HiddenRows::new(),
            )[0]
            .values
            .clone()
        };

        assert_eq!(
            chip("deepseek", UsageReading::Used),
            vec![String::from("12%")]
        );
        assert_eq!(
            chip("deepseek", UsageReading::Left),
            vec![String::from("88%")]
        );
        assert_eq!(
            chip("openrouter", UsageReading::Left),
            vec![String::from("$4 of $10")]
        );
    }

    /// In the popover's Left reading the menu bar says what is left, like the
    /// tab that selects the chip: Z.AI's monthly window at 18% used reads
    /// `82%`. The chip still picks the most-used window, so `82%` is the
    /// tightest one. A value with no percent reads the same either way.
    #[test]
    fn menu_bar_text_follows_the_left_reading() {
        let report = json!({"primary":null, "entries":[
            entry("zai", "zai", &[("Session (5h)", 0.0), ("Weekly", 0.0), ("MCP tools (monthly)", 18.0)]),
            {"id":"openrouter", "status":"ready", "sections":[
                {"type":"metric", "label":"Balance", "value":"$40"}]},
        ]});
        let content = super::super::strip::content_from_payload(
            &report,
            &super::super::strip::Stars::new(),
            &[],
        );
        let chip = |selected, reading| {
            logo_segments(
                &content,
                &report,
                MenuBarLook::Quattro,
                Some(selected),
                true,
                reading,
                &HiddenRows::new(),
            )[0]
            .values
            .clone()
        };

        assert_eq!(chip("zai", UsageReading::Left), vec![String::from("82%")]);
        assert_eq!(chip("zai", UsageReading::Used), vec![String::from("18%")]);
        assert_eq!(
            chip("openrouter", UsageReading::Left),
            vec![String::from("$40")]
        );

        let logos = logo_segments(
            &content,
            &report,
            MenuBarLook::Logos,
            None,
            true,
            UsageReading::Left,
            &HiddenRows::new(),
        );
        assert_eq!(logos[0].values, vec![String::from("82%")]);
        assert_eq!(
            tooltip(&content, UsageReading::Left),
            "zai · 100% 100%\nopenrouter · $40"
        );
    }

    /// A metric hidden in the popover's Customize never wins the chip: Z.AI
    /// with Session and Weekly at 0% and the monthly MCP window at 18% hidden
    /// reads `0%` used and `100%` left, as the popover tab does. Unhidden it
    /// still reads `18%`, and a hidden window with the highest percent never
    /// beats the visible ones, whatever they read.
    #[test]
    fn name_look_leaves_out_hidden_metrics() {
        let zai = |windows: &[(&str, f64)]| json!({"primary":null, "entries":[entry("zai", "zai", windows)]});
        let hide = |keys: &[&str]| -> HiddenRows {
            HiddenRows::from([(
                "zai".to_string(),
                keys.iter().map(|k| k.to_string()).collect(),
            )])
        };
        let chip = |report: &Value, hidden: &HiddenRows, reading| {
            logo_segments(
                &starred(&["zai"]),
                report,
                MenuBarLook::Quattro,
                Some("zai"),
                true,
                reading,
                hidden,
            )[0]
            .values
            .clone()
        };
        let mcp_hidden = hide(&["metric:MCP tools (monthly)"]);
        let idle = zai(&[
            ("Session (5h)", 0.0),
            ("Weekly", 0.0),
            ("MCP tools (monthly)", 18.0),
        ]);

        assert_eq!(
            chip(&idle, &mcp_hidden, UsageReading::Used),
            vec![String::from("0%")]
        );
        assert_eq!(
            chip(&idle, &mcp_hidden, UsageReading::Left),
            vec![String::from("100%")]
        );
        assert_eq!(
            chip(&idle, &HiddenRows::new(), UsageReading::Used),
            vec![String::from("18%")]
        );
        // Another provider's hidden key does not touch this one.
        let elsewhere = HiddenRows::from([(
            "openai".to_string(),
            vec!["metric:MCP tools (monthly)".to_string()],
        )]);
        assert_eq!(
            chip(&idle, &elsewhere, UsageReading::Used),
            vec![String::from("18%")]
        );

        let busy = zai(&[
            ("Session (5h)", 30.0),
            ("Weekly", 30.0),
            ("MCP tools (monthly)", 90.0),
        ]);
        assert_eq!(
            chip(&busy, &mcp_hidden, UsageReading::Used),
            vec![String::from("30%")]
        );
        let weekly_busy = zai(&[
            ("Session (5h)", 10.0),
            ("Weekly", 55.0),
            ("MCP tools (monthly)", 90.0),
        ]);
        assert_eq!(
            chip(&weekly_busy, &mcp_hidden, UsageReading::Left),
            vec![String::from("45%")]
        );
    }

    /// With every metric of the selected provider hidden it has no value, so
    /// the chip falls through to the next candidate the way a provider
    /// without a value always has. Logos also leaves out hidden metrics.
    #[test]
    fn name_look_skips_a_provider_whose_metrics_are_all_hidden() {
        let report = two_entries(None);
        let hidden = HiddenRows::from([("openai".to_string(), vec!["metric:Session".to_string()])]);
        let segments = |look, hidden: &HiddenRows| {
            logo_segments(
                &two_groups(),
                &report,
                look,
                Some("openai"),
                true,
                UsageReading::Used,
                hidden,
            )
        };

        let chip = segments(MenuBarLook::Quattro, &hidden);
        assert_eq!(chip.len(), 1);
        assert_eq!(chip[0].slug, "anthropic");
        assert_eq!(chip[0].values, vec![String::from("54%")]);
        assert_eq!(
            segments(MenuBarLook::Quattro, &HiddenRows::new())[0].slug,
            "openai"
        );
        assert_eq!(segments(MenuBarLook::Logos, &hidden).len(), 1);

        let only =
            json!({"primary":null, "entries":[entry("openai", "gpt", &[("Session", 100.0)])]});
        let alone = logo_segments(
            &starred(&["openai"]),
            &only,
            MenuBarLook::Quattro,
            Some("openai"),
            true,
            UsageReading::Used,
            &hidden,
        );
        assert!(alone.is_empty());
    }

    /// The popover sends `show_as` with every `strip` message; an older or
    /// unknown value keeps the used reading the menu bar always had.
    #[test]
    fn usage_reading_comes_from_the_strip_ipc() {
        let reading = |value: Value| UsageReading::from_strip_ipc(&value);
        assert_eq!(reading(json!({"show_as":"left"})), UsageReading::Left);
        assert_eq!(reading(json!({"show_as":"used"})), UsageReading::Used);
        assert_eq!(reading(json!({"show_as":"sideways"})), UsageReading::Used);
        assert_eq!(reading(json!({})), UsageReading::Used);
    }

    /// The Chart glyph's bars fill with the consumed share in every reading:
    /// the popover's Left mode changes its chip text, never its ink. A value
    /// headline's bar is its percent; its chip text keeps the report figure.
    #[test]
    fn chart_fractions_fill_with_the_used_share_in_both_readings() {
        let report = json!({"primary":null, "entries":[
            entry("zai", "zai", &[("Session (5h)", 0.0), ("Weekly", 40.0)]),
            {"id":"anthropic", "status":"ready", "sections":[
                {"type":"metric", "label":"Weekly", "percent":100, "value":"100%"}]},
            {"id":"openrouter", "status":"ready", "sections":[
                {"type":"metric", "label":"Credits", "value":"$4", "headline":"value", "percent":40}]},
        ]});
        let content = super::super::strip::content_from_payload(
            &report,
            &super::super::strip::Stars::new(),
            &[],
        );
        assert_eq!(chart_fractions(&content), vec![0.0, 0.4, 1.0, 0.4]);
        // The reading still owns the text: Left reads the remaining share,
        // a value headline keeps its figure.
        let (_, _, zai) = &content.groups[0];
        assert_eq!(metric_text(&zai[1], UsageReading::Left), "60%");
        let (_, _, openrouter) = &content.groups[2];
        assert_eq!(metric_text(&openrouter[0], UsageReading::Left), "$4");
    }

    /// A row with no percent has no consumed share to fill with: it draws an
    /// empty bar, never a full one that a blind `1.0 - fraction` would paint.
    #[test]
    fn chart_fractions_leave_a_percent_less_row_empty() {
        let report = json!({"primary":null, "entries":[
            {"id":"openrouter", "status":"ready", "sections":[
                {"type":"metric", "label":"Balance", "value":"$40"},
                {"type":"metric", "label":"Credits", "value":"$4", "headline":"value"}]},
        ]});
        let content = super::super::strip::content_from_payload(
            &report,
            &super::super::strip::Stars::new(),
            &[],
        );
        assert_eq!(chart_fractions(&content), vec![0.0, 0.0]);
    }
}
