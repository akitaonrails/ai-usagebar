//! MartLLM prepaid wallet. The API has no quota window.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::cache::{Cache, acquire_lock_async, human_backoff};
use crate::error::{AppError, Result};
use crate::format::{placeholders, substitute, updated_at_hm, usd};
use crate::outcome::{Outcome, fallback};
use crate::pango::{balance_severity, color_span, escape, severity_color};
use crate::theme::Theme;
use crate::tooltip::{Line, render_bordered};
use crate::usage::{MartLlmSnapshot, VendorSnapshot, finite_amount};
use crate::vendor::{MAX_BODY_BYTES, RenderOpts, VendorId, VendorOutcome, read_body_capped};
use crate::waybar::{Class, WaybarOutput};

const BALANCE_URL: &str = "https://api.martllm.com/v1/balance";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_FORMAT: &str = "{ull_balance}";

type FetchOutcome = Outcome<MartLlmSnapshot>;

impl From<FetchOutcome> for VendorOutcome {
    fn from(outcome: FetchOutcome) -> Self {
        outcome.map(VendorSnapshot::MartLlm)
    }
}

pub(crate) async fn fetch_snapshot(
    client: &reqwest::Client,
    key: &str,
    cache: &Cache,
    ttl: Duration,
) -> Result<FetchOutcome> {
    fetch_snapshot_at(client, key, cache, ttl, BALANCE_URL).await
}

async fn fetch_snapshot_at(
    client: &reqwest::Client,
    key: &str,
    cache: &Cache,
    ttl: Duration,
    endpoint: &str,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;
    if let Some(bytes) = cache.fresh_payload(ttl)?
        && let Ok(snapshot) = parse_balance(&bytes)
    {
        return Ok(Outcome::cached(snapshot, cache, false));
    }

    // Do not retry a rate-limited endpoint because the cached JSON was invalid.
    if let Some(remaining) = cache.backoff_remaining() {
        return Err(AppError::Http {
            status: 429,
            body: format!("rate limited; next attempt in {}", human_backoff(remaining)),
        });
    }

    let request = async {
        let response = client
            .get(endpoint)
            .bearer_auth(key)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            // Error bodies are not needed to render this provider's diagnostic.
            return Err(AppError::Http {
                status: status.as_u16(),
                body: "MartLLM balance request failed".into(),
            });
        }
        parse_balance(&read_body_capped(response, MAX_BODY_BYTES).await?)
    };
    let result = tokio::time::timeout(REQUEST_TIMEOUT, request)
        .await
        .unwrap_or_else(|_| {
            Err(AppError::Transport(
                "MartLLM balance request timed out".into(),
            ))
        });

    match result {
        Ok(snapshot) => {
            // Persist the three documented amounts, never arbitrary response fields.
            cache.write_payload(&serde_json::to_vec(&snapshot)?)?;
            Ok(Outcome::fresh(snapshot))
        }
        Err(error) if error.is_transient() => fallback(cache, None, error, parse_balance),
        Err(error) => {
            cache.mark_stale();
            let (status, message) = match &error {
                AppError::Http { status, body } => (*status, body.clone()),
                other => (0, other.to_string()),
            };
            let diagnostic = cache.write_last_error(status, &message);
            fallback(cache, Some(diagnostic), error, parse_balance)
        }
    }
}

fn parse_balance(bytes: &[u8]) -> Result<MartLlmSnapshot> {
    let snapshot: MartLlmSnapshot = serde_json::from_slice(bytes).map_err(|_| {
        AppError::Schema("MartLLM response did not contain three USD amounts".into())
    })?;
    for (field, value) in [
        ("available_usd", snapshot.available_usd),
        ("balance_usd", snapshot.balance_usd),
        ("held_usd", snapshot.held_usd),
    ] {
        finite_amount("MartLLM", field, value)?;
    }
    if snapshot.held_usd < 0.0 {
        return Err(AppError::Schema("MartLLM reported a negative hold".into()));
    }
    Ok(snapshot)
}

pub(crate) fn build_placeholders(snap: &MartLlmSnapshot) -> HashMap<&'static str, String> {
    let vendor = VendorId::MartLlm;
    placeholders(vec![
        ("icon", vendor.bar_icon().into()),
        ("vendor_short", vendor.short_name().into()),
        ("plan", vendor.display_name().into()),
        ("ull_balance", usd(snap.available_usd)),
        ("ull_total", usd(snap.balance_usd)),
        ("ull_held", usd(snap.held_usd)),
    ])
}

pub(crate) fn render(
    outcome: &FetchOutcome,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let snap = &outcome.snapshot;
    let values = build_placeholders(snap);
    let mut text = substitute(opts.format.as_deref().unwrap_or(DEFAULT_FORMAT), &values);
    if outcome.stale {
        text.push_str(" ⏸");
    }
    let severity = balance_severity(snap.available_usd, "USD");
    let color = severity_color(severity, theme);
    let icon = opts
        .icon
        .as_deref()
        .filter(|icon| !icon.is_empty())
        .map_or_else(String::new, |icon| format!("{icon} "));
    let tooltip = if let Some(format) = opts.tooltip_format.as_deref() {
        substitute(format, &values)
    } else {
        let mut lines = vec![
            Line::Center(VendorId::MartLlm.display_name().into()),
            Line::Sep,
            Line::Body(format!(
                "Available balance: {}",
                escape(&usd(snap.available_usd))
            )),
        ];
        if snap.held_usd > 0.0 {
            lines.push(Line::Body(format!(
                "Total balance: {} · Held for requests: {}",
                escape(&usd(snap.balance_usd)),
                escape(&usd(snap.held_usd)),
            )));
        }
        if let Some((status, message)) = &outcome.last_error {
            let diagnostic = if *status == 0 {
                escape(message)
            } else {
                format!("HTTP {status}: {}", escape(message))
            };
            lines.push(Line::Body(diagnostic));
        }
        lines.push(Line::Body(format!(
            "Updated {}",
            updated_at_hm(now, outcome.cache_age)
        )));
        render_bordered(&lines, theme)
    };
    WaybarOutput {
        text: color_span(color, &format!("{icon}{text}")),
        tooltip,
        class: Class::from(severity),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BALANCE: &[u8] =
        br#"{"object":"balance","balance_usd":12.4,"held_usd":0.35,"available_usd":12.05}"#;

    #[test]
    fn parses_authoritative_available_balance_and_rejects_incomplete_amounts() {
        let snap = parse_balance(BALANCE).unwrap();
        assert_eq!(snap.available_usd, 12.05);
        assert_eq!(build_placeholders(&snap)["ull_total"], "$12.40");
        for invalid in [
            br#"{"balance_usd":12.4,"held_usd":0.35}"#.as_slice(),
            br#"{"balance_usd":12.4,"held_usd":"0.35","available_usd":12.05}"#,
            br#"{"balance_usd":1e500,"held_usd":0.35,"available_usd":12.05}"#,
            br#"{"balance_usd":12.4,"held_usd":-0.35,"available_usd":12.05}"#,
        ] {
            assert!(matches!(parse_balance(invalid), Err(AppError::Schema(_))));
        }
        let debt = parse_balance(br#"{"balance_usd":-2,"held_usd":0,"available_usd":-2}"#).unwrap();
        assert_eq!(build_placeholders(&debt)["ull_balance"], "-$2.00");
    }

    #[tokio::test]
    async fn fetches_only_balance_and_keeps_it_stale_on_auth_failure() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().join("martllm"));
        let mut server = mockito::Server::new_async().await;
        let endpoint = format!("{}/v1/balance", server.url());
        let client = crate::widget::run::http_client().unwrap();
        let ok = server
            .mock("GET", "/v1/balance")
            .match_header("authorization", "Bearer test-key")
            .with_body(
                r#"{"balance_usd":12.4,"held_usd":0.35,"available_usd":12.05,"extra":"test-key"}"#,
            )
            .create_async()
            .await;
        let fresh = fetch_snapshot_at(&client, "test-key", &cache, Duration::ZERO, &endpoint)
            .await
            .unwrap();
        assert!(!fresh.stale);
        assert_eq!(fresh.snapshot.available_usd, 12.05);
        ok.assert_async().await;
        ok.remove_async().await;
        let cached = cache.maybe_payload().unwrap().unwrap();
        let fields: serde_json::Value = serde_json::from_slice(&cached).unwrap();
        assert_eq!(fields.as_object().unwrap().len(), 3);
        assert!(!String::from_utf8(cached).unwrap().contains("test-key"));
        let auth = server
            .mock("GET", "/v1/balance")
            .with_status(401)
            .with_body("test-key")
            .create_async()
            .await;
        let stale = fetch_snapshot_at(&client, "test-key", &cache, Duration::ZERO, &endpoint)
            .await
            .unwrap();
        assert!(stale.stale);
        assert_eq!(stale.snapshot, fresh.snapshot);
        assert_eq!(
            stale.last_error.unwrap(),
            (401, crate::error::AUTH_FAILURE_MESSAGE.into())
        );
        auth.assert_async().await;
    }

    #[tokio::test]
    async fn corrupt_cached_balance_does_not_bypass_backoff() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().join("martllm"));
        cache.write_payload(b"{}").unwrap();
        cache.note_rate_limit_at(std::time::SystemTime::now());
        let mut server = mockito::Server::new_async().await;
        let endpoint = format!("{}/v1/balance", server.url());
        let mock = server
            .mock("GET", "/v1/balance")
            .expect(0)
            .create_async()
            .await;
        let client = crate::widget::run::http_client().unwrap();
        assert!(matches!(
            fetch_snapshot_at(
                &client,
                "test-key",
                &cache,
                Duration::from_secs(300),
                &endpoint
            )
            .await,
            Err(AppError::Http { status: 429, .. })
        ));
        mock.assert_async().await;
    }
}
