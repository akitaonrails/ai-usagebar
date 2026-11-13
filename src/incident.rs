use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cache::Cache;
use crate::display::sanitize_untrusted_line;
use crate::tui::app::TabId;
use crate::vendor::{MAX_BODY_BYTES, VendorId, read_body_capped};

/// What a provider's official status page currently flags as abnormal,
/// attached to a report entry or TUI tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
    /// The status page's address: a constant of this build, never a link
    /// taken from the page's response.
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

/// Whether `title` (lower case) carries `needle` as whole words: `ide` is in
/// "IDE unavailable" and not in "fix identified", `api` not in "rapid".
fn title_names(title: &str, needle: &str) -> bool {
    let words: Vec<&str> = title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let wanted: Vec<&str> = needle.split(' ').collect();
    words.windows(wanted.len()).any(|run| run == wanted)
}

fn incident_applies_to_vendor(vendor: VendorId, inc: &RawIncident) -> bool {
    let status = inc.status.trim().to_ascii_lowercase();
    if matches!(status.as_str(), "resolved" | "postmortem" | "completed") {
        return false;
    }
    if inc.components.is_empty() {
        let title = inc.name.to_lowercase();
        return title_needles(vendor)
            .iter()
            .any(|needle| title_names(&title, needle));
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

fn clamp_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

/// A page's text as one sanitized line: it is shown beside a tab name.
fn headline(text: &str) -> String {
    clamp_chars(&sanitize_untrusted_line(text), MAX_INCIDENT_TITLE_CHARS)
}

/// Parse a Statuspage `/api/v2/summary.json` body for `vendor`: `None` when
/// every component of the vendor is operational and no open incident applies.
/// The mark links to `page_url`, the status page itself: the page shows its
/// open incident first, and a link taken from the response would be
/// third-party text handed to whatever opens it.
fn parse_summary_json(vendor: VendorId, raw_json: &str, page_url: &str) -> Option<IncidentBanner> {
    let parsed: SummaryResponse = serde_json::from_str(raw_json).ok()?;
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

    Some(IncidentBanner {
        severity: status_for_rank(rank).to_string(),
        title,
        url: page_url.to_string(),
    })
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

/// One async lock per status page, by cache key.
type PageTurns = Mutex<HashMap<&'static str, Arc<tokio::sync::Mutex<()>>>>;

/// The lock that lets one caller at a time read or fetch a status page. The
/// accounts of a provider refresh together and several providers share a
/// page: the second caller must find what the first one cached instead of
/// sending the same request beside it. Pages do not wait for each other.
fn page_turn(endpoint: StatusEndpoint) -> Arc<tokio::sync::Mutex<()>> {
    static TURNS: OnceLock<PageTurns> = OnceLock::new();
    TURNS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(endpoint.cache_key)
        .or_default()
        .clone()
}

/// The cached summary body while it is younger than the TTL. Deliberately not
/// [`Cache::fresh_payload`]: under an armed backoff that one serves the last
/// payload whatever its age, which is right for a usage figure and would keep
/// an incident on screen long after the page stopped answering.
fn cached_status_body(cache: &Cache) -> Option<String> {
    if cache.payload_age()? >= INCIDENT_CACHE_TTL {
        return None;
    }
    String::from_utf8(cache.maybe_payload().ok()??).ok()
}

/// The status page's summary body: from the disk cache while it is fresh, then
/// from the network. A failed request arms the cache's backoff, which is on
/// disk, so neither this process nor the next one-shot `usage --json` pays the
/// timeout again until it lapses. A failure is never read as "no incident" and
/// never answered with an older body: it yields `None`, and so no mark.
async fn fetch_raw_status_json(
    client: &reqwest::Client,
    endpoint: StatusEndpoint,
    cache: &Cache,
) -> Option<String> {
    let turn = page_turn(endpoint);
    let _held = turn.lock().await;
    if let Some(body) = cached_status_body(cache) {
        return Some(body);
    }
    if cache.backoff_remaining().is_some() {
        return None;
    }
    let body = request_summary(client, endpoint).await;
    match &body {
        // A successful write also ends any earlier backoff.
        Some(text) => drop(cache.write_payload(text.as_bytes())),
        None => cache.note_rate_limit_at(std::time::SystemTime::now()),
    }
    body
}

/// What the status page of `tab`'s provider flags as abnormal right now.
/// Never fails the caller: `None` when nothing is flagged, when the page could
/// not be reached, and for a provider without a checked status source, which
/// includes every custom one.
pub async fn fetch_for_tab(client: &reqwest::Client, tab: &TabId) -> Option<IncidentBanner> {
    let vendor = tab.vendor_id()?;
    let endpoint = endpoint_for_vendor(vendor)?;
    let cache = Cache::for_vendor_account("incidents", endpoint.cache_key).ok()?;
    let raw = fetch_raw_status_json(client, endpoint, &cache).await?;
    parse_summary_json(vendor, &raw, endpoint.page_url)
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
        // The link is the status page, whatever link the incident carries.
        assert_eq!(banner.url, "https://status.claude.com/");
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
        assert_eq!(banner.url, "https://status.openai.com/");
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
        assert_eq!(banner.url, "https://www.githubstatus.com/");
    }

    const FLAGGED_PAGE: &str =
        r#"{"components": [{"name": "Claude Code", "status": "major_outage"}]}"#;

    /// A status page served by `server`, cached under a temporary directory:
    /// the policy under test never sees a real page or a real cache.
    fn mock_page(server: &mockito::ServerGuard, key: &'static str) -> StatusEndpoint {
        StatusEndpoint {
            cache_key: key,
            api_url: Box::leak(format!("{}/summary.json", server.url()).into_boxed_str()),
            page_url: "https://status.example/",
        }
    }

    /// The page's summary endpoint, answering `status` and expecting `requests`.
    async fn summary_mock(
        server: &mut mockito::ServerGuard,
        status: usize,
        requests: usize,
    ) -> mockito::Mock {
        server
            .mock("GET", "/summary.json")
            .with_status(status)
            .with_body(FLAGGED_PAGE)
            .expect(requests)
            .create_async()
            .await
    }

    fn backdate_payload(cache: &Cache, age: Duration) {
        std::fs::File::options()
            .write(true)
            .open(cache.payload_path())
            .and_then(|file| file.set_modified(std::time::SystemTime::now() - age))
            .expect("backdate the cached status body");
    }

    #[tokio::test]
    async fn a_cached_body_inside_the_ttl_is_served_without_a_request() {
        let mut server = mockito::Server::new_async().await;
        let page = summary_mock(&mut server, 200, 0).await;
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().join("page"));
        cache.write_payload(FLAGGED_PAGE.as_bytes()).unwrap();

        let endpoint = mock_page(&server, "test-fresh-body");
        let body = fetch_raw_status_json(&reqwest::Client::new(), endpoint, &cache).await;

        assert_eq!(body.as_deref(), Some(FLAGGED_PAGE));
        page.assert_async().await;
    }

    #[tokio::test]
    async fn a_stale_body_is_not_served_while_a_failure_backoff_is_armed() {
        // The page stopped answering after it last flagged something. What it
        // said two hours ago is not its status now: no body, so no mark.
        let mut server = mockito::Server::new_async().await;
        let page = summary_mock(&mut server, 200, 0).await;
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().join("page"));
        cache.write_payload(FLAGGED_PAGE.as_bytes()).unwrap();
        backdate_payload(&cache, Duration::from_secs(2 * 60 * 60));
        cache.note_rate_limit_at(std::time::SystemTime::now());

        let endpoint = mock_page(&server, "test-stale-body");
        let body = fetch_raw_status_json(&reqwest::Client::new(), endpoint, &cache).await;

        assert_eq!(body, None);
        page.assert_async().await;
    }

    #[tokio::test]
    async fn a_failed_request_is_remembered_on_disk_and_not_repeated() {
        let mut server = mockito::Server::new_async().await;
        let page = summary_mock(&mut server, 503, 1).await;
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().join("page"));
        let endpoint = mock_page(&server, "test-failed-request");
        let client = reqwest::Client::new();

        assert_eq!(fetch_raw_status_json(&client, endpoint, &cache).await, None);
        assert!(cache.backoff_remaining().is_some());
        assert_eq!(fetch_raw_status_json(&client, endpoint, &cache).await, None);

        page.assert_async().await;
    }

    #[tokio::test]
    async fn an_expired_body_is_fetched_again_and_cached_for_the_next_caller() {
        let mut server = mockito::Server::new_async().await;
        let page = summary_mock(&mut server, 200, 1).await;
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().join("page"));
        cache.write_payload(b"{}").unwrap();
        backdate_payload(&cache, INCIDENT_CACHE_TTL + Duration::from_secs(1));
        let endpoint = mock_page(&server, "test-expired-body");
        let client = reqwest::Client::new();

        let first = fetch_raw_status_json(&client, endpoint, &cache).await;
        let second = fetch_raw_status_json(&client, endpoint, &cache).await;

        assert_eq!(first.as_deref(), Some(FLAGGED_PAGE));
        assert_eq!(second.as_deref(), Some(FLAGGED_PAGE));
        page.assert_async().await;
    }

    #[tokio::test]
    async fn accounts_refreshing_together_share_one_request() {
        let mut server = mockito::Server::new_async().await;
        let page = summary_mock(&mut server, 200, 1).await;
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().join("page"));
        let endpoint = mock_page(&server, "test-shared-request");
        let client = reqwest::Client::new();

        let (first, second, third) = tokio::join!(
            fetch_raw_status_json(&client, endpoint, &cache),
            fetch_raw_status_json(&client, endpoint, &cache),
            fetch_raw_status_json(&client, endpoint, &cache),
        );

        for body in [first, second, third] {
            assert_eq!(body.as_deref(), Some(FLAGGED_PAGE));
        }
        page.assert_async().await;
    }

    #[test]
    fn a_title_names_a_product_only_by_whole_words() {
        let summary = |name: &str| {
            format!(
                r#"{{"incidents": [{{"name": "{name}", "status": "investigating",
                    "impact": "minor"}}]}}"#
            )
        };
        let marks = |vendor: VendorId, name: &str| {
            parse_summary_json(vendor, &summary(name), "https://status.example/").is_some()
        };
        // `ide` inside "identified", `cli` inside "clients", `api` inside "rapid".
        assert!(!marks(
            VendorId::Cursor,
            "Automations delayed, fix identified"
        ));
        assert!(!marks(VendorId::Cursor, "Grok Bot errors for some clients"));
        assert!(!marks(VendorId::Anthropic, "Rapid growth in sign-ups"));
        assert!(marks(VendorId::Cursor, "IDE unavailable"));
        assert!(marks(VendorId::Cursor, "Elevated errors in the CLI"));
        assert!(marks(VendorId::Anthropic, "Elevated API error rates"));
        assert!(marks(VendorId::Anthropic, "Claude Code: sign-in failing"));
    }

    #[test]
    fn a_title_is_one_sanitized_line() {
        let json = r#"{"components": [], "incidents": [{
            "name": "Copilot\ndown\u001b[31m \u202eevil", "status": "investigating",
            "impact": "minor", "components": [{"name": "Copilot"}]}]}"#;
        let banner = parse_summary_json(VendorId::Copilot, json, ENDPOINT_GITHUB.page_url)
            .expect("should match Copilot component");
        assert_eq!(banner.title, "Copilot down[31m evil");
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
