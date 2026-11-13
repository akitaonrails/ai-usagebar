use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::cache::Cache;
use crate::display::sanitize_untrusted_field;
use crate::tui::app::TabId;
use crate::vendor::{MAX_BODY_BYTES, VendorId, read_body_capped};

/// What a provider's official status page currently flags as abnormal,
/// attached to a report entry or TUI tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncidentBanner {
    /// The status the page itself shows, in Statuspage's own vocabulary:
    /// `"degraded_performance"`, `"partial_outage"`, `"major_outage"` or
    /// `"under_maintenance"`. It is the worst status among the provider's
    /// components; an open incident that names no component status
    /// contributes its declared impact on the same scale.
    pub severity: String,
    /// Sanitized headline: the open incident's name, or the affected
    /// component and its status when the page lists no incident for it.
    pub title: String,
    /// Validated `https://` URL to the incident or status page.
    pub url: String,
}

#[derive(Debug, Clone, Copy)]
struct StatusEndpoint {
    cache_key: &'static str,
    api_url: &'static str,
    page_url: &'static str,
}

const ENDPOINT_CLAUDE: StatusEndpoint = StatusEndpoint {
    cache_key: "status.claude.com",
    api_url: "https://status.claude.com/api/v2/summary.json",
    page_url: "https://status.claude.com/",
};

const ENDPOINT_OPENAI: StatusEndpoint = StatusEndpoint {
    cache_key: "status.openai.com",
    api_url: "https://status.openai.com/api/v2/summary.json",
    page_url: "https://status.openai.com/",
};

const ENDPOINT_CURSOR: StatusEndpoint = StatusEndpoint {
    cache_key: "status.cursor.com",
    // status.cursor.com answers with a redirect to this host; asking it
    // directly keeps the request on a single, named origin.
    api_url: "https://status.x.ai/api/compat/summary.json",
    page_url: "https://status.cursor.com/",
};

const ENDPOINT_GITHUB: StatusEndpoint = StatusEndpoint {
    cache_key: "www.githubstatus.com",
    api_url: "https://www.githubstatus.com/api/v2/summary.json",
    page_url: "https://www.githubstatus.com/",
};

const INCIDENT_CACHE_TTL: Duration = Duration::from_secs(300);
const INCIDENT_HTTP_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_INCIDENT_TITLE_CHARS: usize = 140;

fn endpoint_for_vendor(vendor: VendorId) -> Option<StatusEndpoint> {
    match vendor {
        VendorId::Anthropic | VendorId::AnthropicApi => Some(ENDPOINT_CLAUDE),
        VendorId::Openai => Some(ENDPOINT_OPENAI),
        VendorId::Cursor => Some(ENDPOINT_CURSOR),
        VendorId::Copilot => Some(ENDPOINT_GITHUB),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct SummaryResponse {
    #[serde(default)]
    components: Vec<RawComponent>,
    #[serde(default)]
    incidents: Vec<RawIncident>,
}

#[derive(Debug, Deserialize)]
struct RawIncident {
    #[serde(default)]
    name: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    impact: String,
    #[serde(default)]
    shortlink: String,
    #[serde(default)]
    components: Vec<RawComponent>,
}

#[derive(Debug, Deserialize)]
struct RawComponent {
    #[serde(default)]
    name: String,
    #[serde(default)]
    status: String,
}

/// The status-page components a vendor's entry depends on, by name (lower
/// case). Named one by one instead of matched by fragment: a status page lists
/// far more than one product, and a neighbour's outage must not colour this
/// vendor. `None` means the vendor has no checked status source.
fn vendor_components(vendor: VendorId) -> Option<&'static [&'static str]> {
    match vendor {
        // The subscription entry is Claude Code's reading, and Claude Code
        // talks to the API. claude.ai, Cowork, the Console and the cloud
        // platforms are other products on the same page.
        VendorId::Anthropic => Some(&["claude code", "claude api"]),
        VendorId::AnthropicApi => Some(&["claude api", "claude console"]),
        VendorId::Openai => Some(&["codex in chatgpt desktop", "login"]),
        VendorId::Cursor => Some(&["ide", "cli", "cursor.com", "third-party models"]),
        VendorId::Copilot => Some(&["copilot", "copilot ai model providers"]),
        _ => None,
    }
}

/// The names from [`vendor_components`] that no component of this status-page
/// summary carries. Matching is by exact name, so a page that renames a
/// component stops marking its vendor without any error: a non-empty result
/// against the live page is that drift. `None` when the body does not parse
/// or the vendor has no status source.
///
/// ```
/// use ai_usagebar::incident::unmatched_components;
/// use ai_usagebar::vendor::VendorId;
///
/// let page = r#"{"components": [{"name": "Copilot", "status": "operational"}]}"#;
/// assert_eq!(
///     unmatched_components(VendorId::Copilot, page),
///     Some(vec!["copilot ai model providers"])
/// );
/// ```
pub fn unmatched_components(vendor: VendorId, raw_json: &str) -> Option<Vec<&'static str>> {
    let parsed: SummaryResponse = serde_json::from_str(raw_json).ok()?;
    let wanted = vendor_components(vendor)?;
    Some(
        wanted
            .iter()
            .copied()
            .filter(|w| !parsed.components.iter().any(|c| component_is(&c.name, w)))
            .collect(),
    )
}

/// The status-page summary URL read for `vendor`, if it has one.
pub fn status_summary_url(vendor: VendorId) -> Option<&'static str> {
    endpoint_for_vendor(vendor).map(|endpoint| endpoint.api_url)
}

/// Whether `name` is `wanted`, allowing the parenthesised address some pages
/// append: `Claude API (api.anthropic.com)` is `claude api`.
fn component_is(name: &str, wanted: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    name == wanted
        || name
            .strip_prefix(wanted)
            .is_some_and(|rest| rest.trim_start().starts_with('('))
}

fn component_applies_to_vendor(vendor: VendorId, component: &RawComponent) -> bool {
    vendor_components(vendor)
        .is_some_and(|wanted| wanted.iter().any(|w| component_is(&component.name, w)))
}

/// Words that tie an incident to `vendor` by its title, used only when the
/// incident names no component at all and so cannot be placed otherwise.
fn title_needles(vendor: VendorId) -> &'static [&'static str] {
    match vendor {
        VendorId::Anthropic => &["claude code", "api"],
        VendorId::AnthropicApi => &["api", "console"],
        VendorId::Openai => &["codex"],
        VendorId::Cursor => &["cursor", "ide", "cli"],
        VendorId::Copilot => &["copilot"],
        _ => &[],
    }
}

fn incident_applies_to_vendor(vendor: VendorId, inc: &RawIncident) -> bool {
    let status = inc.status.trim().to_ascii_lowercase();
    if matches!(status.as_str(), "resolved" | "postmortem" | "completed") {
        return false;
    }
    if inc.components.is_empty() {
        let title = inc.name.to_ascii_lowercase();
        return title_needles(vendor)
            .iter()
            .any(|needle| title.contains(needle));
    }
    inc.components
        .iter()
        .any(|component| component_applies_to_vendor(vendor, component))
}

/// Severity rank of one Statuspage component status; `None` for
/// `operational` and for any status this build does not know.
fn component_status_rank(status: &str) -> Option<u8> {
    match status.trim().to_ascii_lowercase().as_str() {
        "major_outage" => Some(4),
        "partial_outage" => Some(3),
        "degraded_performance" => Some(2),
        "under_maintenance" => Some(1),
        _ => None,
    }
}

/// An incident's declared impact on the component-status scale, the way
/// Statuspage itself colours it: minor is yellow, major orange, critical red.
fn impact_rank(impact: &str) -> u8 {
    match impact.trim().to_ascii_lowercase().as_str() {
        "critical" => 4,
        "major" => 3,
        "maintenance" => 1,
        _ => 2,
    }
}

/// How bad the incident is *for this vendor*: the worst status among the
/// vendor's affected components, which is what the status page colours, or the
/// incident's declared impact when no affected component carries a status.
/// Impact alone overstates: a `major` incident routinely leaves its
/// components at `degraded_performance`.
fn incident_rank(vendor: VendorId, inc: &RawIncident) -> u8 {
    inc.components
        .iter()
        .filter(|component| component_applies_to_vendor(vendor, component))
        .filter_map(|component| component_status_rank(&component.status))
        .max()
        .unwrap_or_else(|| impact_rank(&inc.impact))
}

fn status_for_rank(rank: u8) -> &'static str {
    match rank {
        4.. => "major_outage",
        3 => "partial_outage",
        2 => "degraded_performance",
        _ => "under_maintenance",
    }
}

fn status_label(rank: u8) -> &'static str {
    match rank {
        4.. => "Major Outage",
        3 => "Partial Outage",
        2 => "Degraded Performance",
        _ => "Under Maintenance",
    }
}

fn sanitize_incident_url(shortlink: &str, fallback_page_url: &str) -> String {
    let trimmed = shortlink.trim();
    if trimmed.starts_with("https://")
        && !trimmed.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        trimmed.to_string()
    } else {
        fallback_page_url.to_string()
    }
}

fn clamp_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

fn headline(text: &str) -> String {
    clamp_chars(&sanitize_untrusted_field(text), MAX_INCIDENT_TITLE_CHARS)
}

/// Parse a Statuspage `/api/v2/summary.json` body for `vendor`: `None` when
/// every component of the vendor is operational and no open incident applies.
pub fn parse_summary_json(
    vendor: VendorId,
    raw_json: &str,
    fallback_page_url: &str,
) -> Option<IncidentBanner> {
    let parsed: SummaryResponse = serde_json::from_str(raw_json).ok()?;
    endpoint_for_vendor(vendor)?;
    let component = parsed
        .components
        .iter()
        .filter(|component| component_applies_to_vendor(vendor, component))
        .filter_map(|component| Some((component_status_rank(&component.status)?, component)))
        .max_by_key(|(rank, _)| *rank);
    let incident = parsed
        .incidents
        .iter()
        .filter(|inc| incident_applies_to_vendor(vendor, inc))
        .map(|inc| (incident_rank(vendor, inc), inc))
        .max_by_key(|(rank, _)| *rank);

    let rank = component
        .iter()
        .map(|(rank, _)| *rank)
        .chain(incident.iter().map(|(rank, _)| *rank))
        .max()?;
    // The incident explains the status, so it names the mark when there is
    // one; a component flagged on its own is named with its status.
    let incident_title = incident
        .map(|(_, inc)| headline(&inc.name))
        .filter(|title| !title.is_empty());
    let title = incident_title
        .or_else(|| {
            component.map(|(rank, c)| headline(&format!("{}: {}", c.name, status_label(rank))))
        })
        .unwrap_or_else(|| format!("{} service incident", vendor.display_name()));
    let url = incident.map_or_else(
        || fallback_page_url.to_string(),
        |(_, inc)| sanitize_incident_url(&inc.shortlink, fallback_page_url),
    );

    Some(IncidentBanner {
        severity: status_for_rank(rank).to_string(),
        title,
        url,
    })
}

type IncidentMemo = HashMap<&'static str, (Instant, Option<String>)>;

/// In-process memo so the accounts of one provider in a single report pass
/// share one status body (or one failure) without repeating disk or HTTP work.
fn mem_cache() -> &'static Mutex<IncidentMemo> {
    static MEM: OnceLock<Mutex<IncidentMemo>> = OnceLock::new();
    MEM.get_or_init(|| Mutex::new(HashMap::new()))
}

fn memoize(endpoint: StatusEndpoint, body: Option<String>) -> Option<String> {
    if let Ok(mut guard) = mem_cache().lock() {
        guard.insert(endpoint.cache_key, (Instant::now(), body.clone()));
    }
    body
}

async fn request_summary(client: &reqwest::Client, endpoint: StatusEndpoint) -> Option<String> {
    let response = client
        .get(endpoint.api_url)
        .timeout(INCIDENT_HTTP_TIMEOUT)
        .header("Accept", "application/json")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = read_body_capped(response, MAX_BODY_BYTES).await.ok()?;
    let text = String::from_utf8(bytes).ok()?;
    serde_json::from_str::<SummaryResponse>(&text).ok()?;
    Some(text)
}

/// The status page's summary body: from this process's memo, then the disk
/// cache, then the network. A failed request arms the cache's backoff, which
/// is on disk, so neither this process nor the next one-shot `usage --json`
/// pays the timeout again until it lapses. A failure is never read as "no
/// incident": it yields `None`, and so no mark.
async fn fetch_raw_status_json(
    client: &reqwest::Client,
    endpoint: StatusEndpoint,
    cache: &Cache,
) -> Option<String> {
    if let Ok(guard) = mem_cache().lock()
        && let Some((at, cached)) = guard.get(endpoint.cache_key)
        && at.elapsed() < INCIDENT_CACHE_TTL
    {
        return cached.clone();
    }
    if let Ok(Some(bytes)) = cache.fresh_payload(INCIDENT_CACHE_TTL)
        && let Ok(body) = String::from_utf8(bytes)
    {
        return memoize(endpoint, Some(body));
    }
    if cache.backoff_remaining().is_some() {
        return memoize(endpoint, None);
    }
    let body = request_summary(client, endpoint).await;
    match &body {
        // A successful write also ends any earlier backoff.
        Some(text) => drop(cache.write_payload(text.as_bytes())),
        None => cache.note_rate_limit_at(std::time::SystemTime::now()),
    }
    memoize(endpoint, body)
}

/// What `vendor`'s status page flags as abnormal right now. Never fails the
/// caller: `None` when nothing is flagged, when the page could not be reached,
/// and for a vendor without a checked status source.
pub async fn fetch_for_vendor(
    client: &reqwest::Client,
    vendor: VendorId,
) -> Option<IncidentBanner> {
    let endpoint = endpoint_for_vendor(vendor)?;
    let cache = Cache::for_vendor_account("incidents", endpoint.cache_key).ok()?;
    let raw = fetch_raw_status_json(client, endpoint, &cache).await?;
    parse_summary_json(vendor, &raw, endpoint.page_url)
}

/// [`fetch_for_vendor`] for a tab; a custom provider has no status source.
pub async fn fetch_for_tab(client: &reqwest::Client, tab: &TabId) -> Option<IncidentBanner> {
    fetch_for_vendor(client, tab.vendor_id()?).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_summary_json_returns_none_when_empty() {
        let json = r#"{"page":{"id":"1"},"incidents":[]}"#;
        assert_eq!(
            parse_summary_json(VendorId::Anthropic, json, ENDPOINT_CLAUDE.page_url),
            None
        );
    }

    #[test]
    fn parse_summary_json_maps_impact_and_picks_highest_severity() {
        let json = r#"{
            "incidents": [
                {
                    "name": "Elevated latency on Claude API",
                    "status": "investigating",
                    "impact": "minor",
                    "shortlink": "https://stspg.io/minor1",
                    "components": [{"name": "Claude API"}]
                },
                {
                    "name": "Claude.ai and Claude Code outage",
                    "status": "identified",
                    "impact": "critical",
                    "shortlink": "https://stspg.io/crit1",
                    "components": [{"name": "Claude Code"}]
                }
            ]
        }"#;
        let banner = parse_summary_json(VendorId::Anthropic, json, ENDPOINT_CLAUDE.page_url)
            .expect("should detect incident");
        assert_eq!(banner.severity, "major_outage");
        assert_eq!(banner.title, "Claude.ai and Claude Code outage");
        assert_eq!(banner.url, "https://stspg.io/crit1");
    }

    #[test]
    fn severity_follows_the_affected_component_status_not_the_declared_impact() {
        // Shape of a real status.claude.com incident: declared `major`, while
        // the page showed both components as "Degraded Performance" (yellow).
        let json = r#"{
            "incidents": [
                {
                    "name": "Elevated errors affecting a connector",
                    "status": "identified",
                    "impact": "major",
                    "shortlink": "https://stspg.io/deg1",
                    "components": [
                        {"name": "Claude Code", "status": "degraded_performance"},
                        {"name": "Claude Cowork", "status": "degraded_performance"}
                    ]
                }
            ]
        }"#;
        let banner = parse_summary_json(VendorId::Anthropic, json, ENDPOINT_CLAUDE.page_url)
            .expect("should detect incident");
        assert_eq!(banner.severity, "degraded_performance");
    }

    #[test]
    fn a_neighbouring_products_incident_does_not_mark_the_vendor() {
        // A real status.claude.com summary: claude.ai and Cowork degraded by a
        // connector incident while Claude Code and the API stayed operational.
        let json = r#"{
            "components": [
                {"name": "claude.ai", "status": "degraded_performance"},
                {"name": "Claude Console (platform.claude.com)", "status": "operational"},
                {"name": "Claude API (api.anthropic.com)", "status": "operational"},
                {"name": "Claude Code", "status": "operational"},
                {"name": "Claude Cowork", "status": "degraded_performance"}
            ],
            "incidents": [
                {
                    "name": "Elevated errors affecting the Microsoft 365 connector",
                    "status": "identified",
                    "impact": "major",
                    "shortlink": "https://stspg.io/m365",
                    "components": [
                        {"name": "claude.ai", "status": "degraded_performance"},
                        {"name": "Claude Cowork", "status": "degraded_performance"}
                    ]
                }
            ]
        }"#;
        for vendor in [VendorId::Anthropic, VendorId::AnthropicApi] {
            assert_eq!(
                parse_summary_json(vendor, json, ENDPOINT_CLAUDE.page_url),
                None
            );
        }
    }

    #[test]
    fn an_incident_without_components_applies_only_when_its_title_names_the_product() {
        let summary = |name: &str| {
            format!(
                r#"{{"incidents": [{{"name": "{name}", "status": "investigating",
                    "impact": "minor", "components": []}}]}}"#
            )
        };
        let unrelated = summary("Elevated errors affecting the Microsoft 365 connector");
        assert_eq!(
            parse_summary_json(VendorId::Openai, &unrelated, ENDPOINT_OPENAI.page_url),
            None
        );
        let codex = summary("Codex tasks failing to start");
        let banner = parse_summary_json(VendorId::Openai, &codex, ENDPOINT_OPENAI.page_url)
            .expect("the title names Codex");
        assert_eq!(banner.severity, "degraded_performance");
    }

    #[test]
    fn severity_takes_the_worst_status_among_the_vendors_own_components() {
        let json = r#"{
            "incidents": [
                {
                    "name": "Incident with Copilot and Actions",
                    "status": "investigating",
                    "impact": "minor",
                    "shortlink": "https://stspg.io/gh2",
                    "components": [
                        {"name": "Actions", "status": "major_outage"},
                        {"name": "Copilot", "status": "degraded_performance"}
                    ]
                },
                {
                    "name": "Copilot chat unavailable",
                    "status": "investigating",
                    "impact": "minor",
                    "shortlink": "https://stspg.io/gh3",
                    "components": [{"name": "Copilot", "status": "partial_outage"}]
                }
            ]
        }"#;
        // Actions' outage is not Copilot's: the first incident is only a
        // warning for Copilot, so the second, a Copilot outage, wins.
        let banner = parse_summary_json(VendorId::Copilot, json, ENDPOINT_GITHUB.page_url)
            .expect("should match Copilot component");
        assert_eq!(banner.severity, "partial_outage");
        assert_eq!(banner.title, "Copilot chat unavailable");
    }

    #[test]
    fn severity_falls_back_to_impact_when_no_component_carries_a_status() {
        for (components, impact, expected) in [
            ("[]", "critical", "major_outage"),
            ("[]", "major", "partial_outage"),
            ("[]", "minor", "degraded_performance"),
            ("[]", "maintenance", "under_maintenance"),
            (r#"[{"name": "IDE"}]"#, "critical", "major_outage"),
            (
                r#"[{"name": "IDE", "status": "operational"}]"#,
                "major",
                "partial_outage",
            ),
            (
                r#"[{"name": "IDE", "status": "under_maintenance"}]"#,
                "major",
                "under_maintenance",
            ),
        ] {
            let json = format!(
                r#"{{"incidents": [{{"name": "Cursor incident", "status": "identified",
                    "impact": "{impact}", "components": {components}}}]}}"#
            );
            let banner = parse_summary_json(VendorId::Cursor, &json, ENDPOINT_CURSOR.page_url)
                .expect("should detect incident");
            assert_eq!(banner.severity, expected, "{components} / {impact}");
        }
    }

    #[test]
    fn an_abnormal_component_marks_the_vendor_without_any_open_incident() {
        let json = r#"{
            "components": [
                {"name": "claude.ai", "status": "operational"},
                {"name": "Claude Code", "status": "partial_outage"},
                {"name": "Claude API (api.anthropic.com)", "status": "degraded_performance"}
            ],
            "incidents": []
        }"#;
        let banner = parse_summary_json(VendorId::Anthropic, json, ENDPOINT_CLAUDE.page_url)
            .expect("an abnormal component is enough");
        assert_eq!(banner.severity, "partial_outage");
        assert_eq!(banner.title, "Claude Code: Partial Outage");
        assert_eq!(banner.url, "https://status.claude.com/");
    }

    #[test]
    fn another_products_abnormal_component_does_not_mark_the_vendor() {
        let json = r#"{
            "components": [
                {"name": "Actions", "status": "major_outage"},
                {"name": "Copilot", "status": "operational"}
            ],
            "incidents": []
        }"#;
        assert_eq!(
            parse_summary_json(VendorId::Copilot, json, ENDPOINT_GITHUB.page_url),
            None
        );
    }

    #[test]
    fn the_worst_of_component_and_incident_sets_the_status_and_the_incident_names_it() {
        let json = r#"{
            "components": [{"name": "Codex in ChatGPT Desktop", "status": "major_outage"}],
            "incidents": [
                {
                    "name": "Codex: elevated error rates",
                    "status": "investigating",
                    "impact": "minor",
                    "shortlink": "https://stspg.io/oai1",
                    "components": []
                }
            ]
        }"#;
        let banner = parse_summary_json(VendorId::Openai, json, ENDPOINT_OPENAI.page_url)
            .expect("should detect status");
        assert_eq!(banner.severity, "major_outage");
        assert_eq!(banner.title, "Codex: elevated error rates");
        assert_eq!(banner.url, "https://stspg.io/oai1");
    }

    #[test]
    fn unmatched_components_names_what_the_page_no_longer_lists() {
        let page = r#"{"components": [
            {"name": "Claude Code", "status": "operational"},
            {"name": "Claude API (api.anthropic.com)", "status": "operational"},
            {"name": "claude.ai", "status": "operational"}
        ]}"#;
        assert_eq!(
            unmatched_components(VendorId::Anthropic, page),
            Some(vec![])
        );
        assert_eq!(
            unmatched_components(VendorId::AnthropicApi, page),
            Some(vec!["claude console"])
        );
        assert_eq!(unmatched_components(VendorId::Zai, page), None);
        assert_eq!(unmatched_components(VendorId::Anthropic, "not-json"), None);
    }

    #[test]
    fn copilot_ignores_unrelated_github_incidents_and_matches_copilot_component() {
        let unrelated = r#"{
            "incidents": [
                {
                    "name": "Incident with GitHub Actions and Pages",
                    "status": "investigating",
                    "impact": "major",
                    "shortlink": "https://stspg.io/gh1",
                    "components": [{"name": "Actions"}, {"name": "Pages"}]
                }
            ]
        }"#;
        assert_eq!(
            parse_summary_json(VendorId::Copilot, unrelated, ENDPOINT_GITHUB.page_url),
            None
        );

        let copilot_incident = r#"{
            "incidents": [
                {
                    "name": "Degraded performance for GitHub services",
                    "status": "monitoring",
                    "impact": "minor",
                    "shortlink": "http://insecure.example/link",
                    "components": [{"name": "Copilot"}]
                }
            ]
        }"#;
        let banner = parse_summary_json(
            VendorId::Copilot,
            copilot_incident,
            ENDPOINT_GITHUB.page_url,
        )
        .expect("should match Copilot component");
        assert_eq!(banner.severity, "degraded_performance");
        assert_eq!(banner.title, "Degraded performance for GitHub services");
        // Non-https shortlink falls back to official https status page URL.
        assert_eq!(banner.url, "https://www.githubstatus.com/");
    }

    #[test]
    fn resolved_incidents_and_invalid_json_return_none() {
        let resolved = r#"{
            "incidents": [
                {
                    "name": "Cursor Tab issue",
                    "status": "resolved",
                    "impact": "major",
                    "shortlink": "https://status.cursor.com/incidents/1",
                    "components": []
                }
            ]
        }"#;
        assert_eq!(
            parse_summary_json(VendorId::Cursor, resolved, ENDPOINT_CURSOR.page_url),
            None
        );
        assert_eq!(
            parse_summary_json(VendorId::Openai, "not-json", ENDPOINT_OPENAI.page_url),
            None
        );
    }
}
