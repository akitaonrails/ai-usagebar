use std::time::Duration;

use serde::Deserialize;

use crate::cache::{Cache, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::ExperientialLabsSnapshot;
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

use super::types::{CachedPayload, Credits, KeyInfo, UsageEvent};

pub const BASE_URL: &str = "https://api.experientiallabs.ai";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

pub type FetchOutcome = crate::outcome::Outcome<ExperientialLabsSnapshot>;

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub credits: String,
    pub key: String,
    pub usage: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            credits: format!("{BASE_URL}/api/v1/credits"),
            key: format!("{BASE_URL}/api/v1/key"),
            usage: format!("{BASE_URL}/api/v1/usage?limit=200"),
        }
    }
}

pub async fn fetch_snapshot(
    client: &reqwest::Client,
    api_key: &str,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;
    let parse_cache = |bytes: &[u8]| -> Result<ExperientialLabsSnapshot> {
        let payload: CachedPayload = serde_json::from_slice(bytes)
            .map_err(|_| AppError::Schema("Experiential Labs usage cache is invalid".into()))?;
        payload.snapshot()
    };

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(snapshot) = parse_cache(&bytes)
    {
        return Ok(crate::outcome::Outcome::cached(snapshot, cache, false));
    }
    // An unparseable cache still must not bypass an active 429 backoff.
    if let Some(remaining) = cache.backoff_remaining() {
        return Err(AppError::Http {
            status: 429,
            body: format!(
                "rate limited; next attempt in {}",
                crate::cache::human_backoff(remaining)
            ),
        });
    }

    match request(client, api_key, endpoints).await {
        Ok((payload, snapshot)) => {
            cache.write_payload(&serde_json::to_vec(&payload)?)?;
            Ok(crate::outcome::Outcome::fresh(snapshot))
        }
        Err(error) if error.is_transient() => {
            crate::outcome::fallback(cache, None, error, parse_cache)
        }
        Err(error) => {
            let code = match &error {
                AppError::Http { status, .. } => *status,
                _ => 0,
            };
            // 401/403 mean the key itself was rejected. Serving the stale
            // payload here would keep rendering a wallet the key can no
            // longer see, so the failure is surfaced instead: the cache is
            // marked stale for the age line, the diagnostic is recorded,
            // and the error propagates. Only transient failures (transport,
            // 5xx, 429) fall back to the cached payload.
            if matches!(code, 401 | 403) {
                cache.mark_stale();
                let _diagnostic = cache.write_last_error(code, &error.to_string());
                return Err(error);
            }
            cache.mark_stale();
            let diagnostic = cache.write_last_error(code, &error.to_string());
            crate::outcome::fallback(cache, Some(diagnostic), error, parse_cache)
        }
    }
}

async fn request(
    client: &reqwest::Client,
    api_key: &str,
    endpoints: &Endpoints,
) -> Result<(CachedPayload, ExperientialLabsSnapshot)> {
    let credits: Credits = get_json(client, api_key, &endpoints.credits, "credits").await?;
    // The key's cap is supplementary: a failure leaves the balance figures up.
    let key: Option<KeyInfo> = get_json(client, api_key, &endpoints.key, "key").await.ok();
    // The settled export is the daily meters' source; a failure here is a
    // real failure and must not silently become an empty day.
    let events: Vec<UsageEvent> = get_json_vec(client, api_key, &endpoints.usage, "usage export").await?;
    let payload = CachedPayload {
        credits,
        key,
        events,
    };
    let snapshot = payload.snapshot()?;
    Ok((payload, snapshot))
}

/// Every management read wraps its payload in `data`; the settled export wraps
/// its row array the same way. One unwrap covers both shapes, and a response
/// with no `data` field is its own payload.
fn unwrap_data(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(ref object) => match object.get("data") {
            Some(serde_json::Value::Object(payload)) => serde_json::Value::Object(payload.clone()),
            Some(serde_json::Value::Array(rows)) => serde_json::Value::Array(rows.clone()),
            _ => value,
        },
        other => other,
    }
}

async fn get_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    api_key: &str,
    url: &str,
    what: &str,
) -> Result<T> {
    let response = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .get(url)
            .bearer_auth(api_key)
            .header(reqwest::header::ACCEPT, "application/json")
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("Experiential Labs {what} request timed out")))??;
    let status = response.status();
    let bytes = read_body_capped(response, MAX_BODY_BYTES).await?;
    if !status.is_success() {
        return Err(http_error(status.as_u16(), &bytes));
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
        AppError::Schema(format!("Experiential Labs {what} response schema mismatch"))
    })?;
    serde_json::from_value(unwrap_data(value))
        .map_err(|_| AppError::Schema(format!("Experiential Labs {what} response schema mismatch")))
}

async fn get_json_vec<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    api_key: &str,
    url: &str,
    what: &str,
) -> Result<Vec<T>> {
    let response = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .get(url)
            .bearer_auth(api_key)
            .header(reqwest::header::ACCEPT, "application/json")
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("Experiential Labs {what} request timed out")))??;
    let status = response.status();
    let bytes = read_body_capped(response, MAX_BODY_BYTES).await?;
    if !status.is_success() {
        return Err(http_error(status.as_u16(), &bytes));
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
        AppError::Schema(format!("Experiential Labs {what} response schema mismatch"))
    })?;
    let rows = match value {
        serde_json::Value::Object(ref object) => match object.get("data") {
            Some(serde_json::Value::Array(rows)) => rows.clone(),
            _ => Vec::new(),
        },
        serde_json::Value::Array(rows) => rows,
        _ => Vec::new(),
    };
    serde_json::from_value(serde_json::Value::Array(rows))
        .map_err(|_| AppError::Schema(format!("Experiential Labs {what} response schema mismatch")))
}

/// RFC 9457 problem body. `code` is the stable field — `detail` is prose and
/// must be sanitized before it can reach a tooltip.
#[derive(Deserialize)]
struct Problem {
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

fn http_error(status: u16, bytes: &[u8]) -> AppError {
    let problem: Option<Problem> = serde_json::from_slice(bytes).ok();
    let (code, detail) = match problem {
        Some(Problem { code, detail }) => (code, detail),
        None => (None, None),
    };
    let code = code.as_deref();
    let detail =
        detail.unwrap_or_else(|| String::from_utf8_lossy(bytes).chars().take(200).collect());
    let detail = crate::display::sanitize_untrusted_line(&detail);
    let body = match code {
        Some("insufficient_scope") => format!(
            "{detail} — the presented key lacks a scope this call needs; create a key with the documented reads at platform.experientiallabs.ai/settings/api-keys"
        ),
        Some("unauthenticated" | "invalid_api_key") => {
            format!(
                "{detail} — check the key, or create a new one at platform.experientiallabs.ai/settings/api-keys"
            )
        }
        _ => detail,
    };
    AppError::Http { status, body }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cache() -> (TempDir, Cache) {
        let dir = TempDir::new().unwrap();
        let cache = Cache::at(dir.path().join("explabs"));
        cache.ensure_dir().unwrap();
        (dir, cache)
    }

    const CREDITS: &str = r#"{"data":{"total_credits":2000,"total_usage":1750}}"#;
    const KEY: &str = r#"{"usage":123.45,"limit":2000,"limit_remaining":1765.55}"#;
    const EVENTS: &str = r#"{"data":[{"created_at":"2026-09-01T12:00:00Z","model":"claude-xpl","input_tokens":1000,"output_tokens":500,"cost_usd":1.75}]}"#;

    fn endpoints(server: &mockito::Server) -> Endpoints {
        Endpoints {
            credits: format!("{}/api/v1/credits", server.url()),
            key: format!("{}/api/v1/key", server.url()),
            usage: format!("{}/api/v1/usage?limit=200", server.url()),
        }
    }

    #[tokio::test]
    async fn sends_bearer_key_and_parses_the_balance() {
        let mut server = mockito::Server::new_async().await;
        let credits = server
            .mock("GET", "/api/v1/credits")
            .match_header("authorization", "Bearer xpl-test")
            .match_header("accept", "application/json")
            .with_status(200)
            .with_body(CREDITS)
            .create_async()
            .await;
        let key = server
            .mock("GET", "/api/v1/key")
            .with_status(200)
            .with_body(KEY)
            .create_async()
            .await;
        let events = server
            .mock("GET", "/api/v1/usage")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body(EVENTS)
            .create_async()
            .await;
        let (_dir, cache) = cache();

        let outcome = fetch_snapshot(
            &reqwest::Client::new(),
            "xpl-test",
            &cache,
            &endpoints(&server),
            Duration::ZERO,
        )
        .await
        .unwrap();

        assert_eq!(outcome.snapshot.balance_usd, Some(250.0));
        assert_eq!(outcome.snapshot.key_limit_usd, Some(2000.0));
        assert_eq!(outcome.snapshot.key_limit_remaining_usd, Some(1765.55));
        assert_eq!(outcome.snapshot.models.len(), 1);
        assert_eq!(outcome.snapshot.models[0].model, "claude-xpl");
        assert_eq!(outcome.snapshot.models[0].input_tokens, 1000);
        assert_eq!(outcome.snapshot.models[0].output_tokens, 500);
        assert!(outcome.off_the_wire());
        credits.assert_async().await;
        key.assert_async().await;
        events.assert_async().await;
    }

    const FIXTURE_CREDITS: &str = include_str!("../../tests/fixtures/explabs/credits.json");
    const FIXTURE_KEY: &str = include_str!("../../tests/fixtures/explabs/key.json");
    const FIXTURE_USAGE: &str = include_str!("../../tests/fixtures/explabs/usage.json");

    #[test]
    fn captured_responses_project_the_documented_shape() {
        let unwrap =
            |raw: &str| unwrap_data(serde_json::from_str(raw).expect("fixture is valid json"));
        let credits: Credits =
            serde_json::from_value(unwrap(FIXTURE_CREDITS)).expect("credits shape");
        let key: KeyInfo = serde_json::from_value(unwrap(FIXTURE_KEY)).expect("key shape");
        let events: Vec<UsageEvent> =
            serde_json::from_value(unwrap(FIXTURE_USAGE)).expect("usage export shape");
        let snapshot = CachedPayload {
            credits,
            key: Some(key),
            events,
        }
        .snapshot()
        .expect("snapshot");

        assert_eq!(snapshot.balance_usd, Some(24.5));
        assert_eq!(snapshot.key_limit_usd, Some(50.0));
        assert_eq!(snapshot.key_limit_remaining_usd, Some(37.66));
        // The per-model rollup spans the whole settled window, heaviest
        // first: the two claude-xpl rows fold into one.
        assert_eq!(snapshot.models.len(), 2);
        assert_eq!(snapshot.models[0].model, "claude-xpl");
        assert_eq!(snapshot.models[0].input_tokens, 876_400);
        assert_eq!(snapshot.models[0].output_tokens, 203_600);
        assert_eq!(snapshot.models[0].cost_usd, 2.0);
        assert_eq!(snapshot.models[1].model, "glm-xpl");
        // The fixture's rows are settled on a past day, so the daily meters
        // stay unset here unless the suite runs on 2026-11-06; the
        // today-stamped projection is covered in `types.rs`.
    }

    #[tokio::test]
    async fn reads_the_key_cap_for_capped_keys() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/v1/credits")
            .with_status(200)
            .with_body(CREDITS)
            .create_async()
            .await;
        server
            .mock("GET", "/api/v1/key")
            .with_status(200)
            .with_body(KEY)
            .create_async()
            .await;
        server
            .mock("GET", "/api/v1/usage")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body(EVENTS)
            .create_async()
            .await;
        let (_dir, cache) = cache();

        let outcome = fetch_snapshot(
            &reqwest::Client::new(),
            "xpl-test",
            &cache,
            &endpoints(&server),
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(outcome.snapshot.key_limit_usd, Some(2000.0));
        assert_eq!(outcome.snapshot.key_limit_remaining_usd, Some(1765.55));
    }
}
