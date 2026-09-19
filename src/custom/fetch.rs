//! Fetch a custom provider: one GET with a static token, projected through
//! the user's pointers.
//!
//! The cache holds the *projected* snapshot, not the response body. Every
//! built-in vendor caches what the wire returned and re-parses it on fallback,
//! which is fine because the vendor owns the schema. Here the schema is
//! whatever the user pointed at: caching the body would keep every field they
//! did not ask for — an account e-mail, an org id — on disk for `MAX_STALE`,
//! and a later config edit would re-project a stale body through new
//! pointers. The projection is small, holds only what the user chose, and
//! round-trips through serde, so the fallback parse is `serde_json::from_slice`.

use std::time::Duration;

use reqwest::header::{HeaderName, HeaderValue};

use crate::cache::{Cache, acquire_lock_async};
use crate::config::CustomProviderConfig;
use crate::display::sanitize_untrusted_line;
use crate::error::{AppError, Result};

use super::types::CustomSnapshot;

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

/// This vendor's [`Outcome`](crate::outcome::Outcome) — the shared shape,
/// specialised to its snapshot.
pub type FetchOutcome = crate::outcome::Outcome<CustomSnapshot>;

pub async fn fetch_snapshot(
    client: &reqwest::Client,
    spec: &CustomProviderConfig,
    api_key: &str,
    cache: &Cache,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse_cache(bytes, cache, false)
    {
        return Ok(outcome);
    }
    // A fresh cache that will not parse is a projection written by an older
    // build or a different pointer set: refetch rather than show it.

    match fetch_live(client, spec, api_key).await {
        Ok(snap) => {
            let bytes = serde_json::to_vec(&snap)?;
            cache.write_payload(&bytes)?;
            Ok(crate::outcome::Outcome::fresh(snap))
        }
        Err(e) if e.is_transient() => fallback_silent(cache, e),
        Err(AppError::Http { status, body }) => {
            cache.mark_stale();
            let last_error = Some(cache.write_last_error(status, &body));
            fallback_with_error(cache, last_error, AppError::Http { status, body })
        }
        Err(e) => {
            cache.mark_stale();
            let last_error = Some(cache.write_last_error(0, &e.to_string()));
            fallback_with_error(cache, last_error, e)
        }
    }
}

fn fallback_silent(cache: &Cache, original: AppError) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, None, original, parse_cache)
}

fn fallback_with_error(
    cache: &Cache,
    last_error: Option<(u16, String)>,
    original: AppError,
) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, last_error, original, parse_cache)
}

fn reuse_cache(bytes: Vec<u8>, cache: &Cache, stale: bool) -> Result<FetchOutcome> {
    let snap = parse_cache(&bytes)?;
    Ok(crate::outcome::Outcome::cached(snap, cache, stale))
}

fn parse_cache(bytes: &[u8]) -> Result<CustomSnapshot> {
    Ok(serde_json::from_slice(bytes)?)
}

async fn fetch_live(
    client: &reqwest::Client,
    spec: &CustomProviderConfig,
    api_key: &str,
) -> Result<CustomSnapshot> {
    let id = spec.id.as_str();
    let mut request = client
        .get(spec.url.as_str())
        .header("Accept", "application/json");
    for (name, value) in &spec.headers {
        request = request.header(header_name(id, name)?, header_value(id, name, value)?);
    }
    // Marked sensitive so reqwest's own debug output redacts it, the same way
    // it treats `Authorization`, whatever header name the user chose.
    let mut auth = header_value(
        id,
        &spec.auth_header,
        &auth_value(&spec.auth_scheme, api_key),
    )?;
    auth.set_sensitive(true);
    request = request.header(header_name(id, &spec.auth_header)?, auth);

    // The timeout message deliberately omits the URL: a user may have put a
    // query-string token in it, and this string ends up in `.last_error`.
    let resp = tokio::time::timeout(HTTP_TIMEOUT, request.send())
        .await
        .map_err(|_| AppError::Transport(format!("custom {id}: request timed out")))??;

    let status = resp.status();
    let bytes = crate::vendor::read_body_capped(resp, crate::vendor::MAX_BODY_BYTES).await?;

    if !status.is_success() {
        let body = sanitize_untrusted_line(&String::from_utf8_lossy(&bytes))
            .chars()
            .take(200)
            .collect();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }

    // serde_json's message carries a line/column, never the text it failed on.
    let body: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("custom {id}: response is not JSON: {e}")))?;
    super::mapping::project(&body, spec)
}

fn auth_value(scheme: &str, api_key: &str) -> String {
    if scheme.is_empty() {
        api_key.to_string()
    } else {
        format!("{scheme} {api_key}")
    }
}

fn header_name(id: &str, name: &str) -> Result<HeaderName> {
    HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| AppError::Other(format!("custom {id}: {name:?} is not a valid header name")))
}

/// The value is never part of the error: for the auth header it is the key.
fn header_value(id: &str, name: &str, value: &str) -> Result<HeaderValue> {
    HeaderValue::from_str(value).map_err(|_| {
        AppError::Other(format!(
            "custom {id}: header {name:?} has a value that is not valid in an HTTP header"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CustomMetricSpec;
    use crate::custom::types::{CustomMetric, CustomText};
    use tempfile::TempDir;

    const KEY: &str = "sk-custom-test-3f9a";

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("custom").join("mytool"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    fn spec_for(server: &mockito::ServerGuard) -> CustomProviderConfig {
        CustomProviderConfig {
            id: "mytool".into(),
            name: "My Tool".into(),
            short_name: "myt".into(),
            enabled: true,
            url: format!("{}/v1/usage", server.url()),
            allow_http: true,
            metrics: vec![CustomMetricSpec {
                label: "Requests".into(),
                used: Some("/requests/used".into()),
                limit: Some("/requests/limit".into()),
                ..CustomMetricSpec::default()
            }],
            ..CustomProviderConfig::default()
        }
    }

    fn body() -> &'static str {
        r#"{"requests": {"used": 25, "limit": 100}}"#
    }

    fn warm_snapshot() -> CustomSnapshot {
        CustomSnapshot {
            plan: Some("Pro".into()),
            metrics: vec![CustomMetric {
                label: "Requests".into(),
                pct: 40,
                footnote: "40 of 100".into(),
                resets_at: None,
                window_secs: None,
            }],
            texts: vec![CustomText {
                label: "Tier".into(),
                value: "gold".into(),
            }],
        }
    }

    async fn fetch(
        spec: &CustomProviderConfig,
        key: &str,
        cache: &Cache,
        ttl: Duration,
    ) -> Result<FetchOutcome> {
        fetch_snapshot(&reqwest::Client::new(), spec, key, cache, ttl).await
    }

    #[tokio::test]
    async fn sends_the_bearer_token_and_returns_a_fresh_projection() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/usage")
            .match_header("authorization", format!("Bearer {KEY}").as_str())
            .match_header("accept", "application/json")
            .with_status(200)
            .with_body(body())
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let out = fetch(&spec_for(&server), KEY, &cache, Duration::ZERO)
            .await
            .unwrap();
        mock.assert_async().await;
        assert!(!out.stale);
        assert_eq!(out.last_error, None);
        assert_eq!(out.snapshot.metrics[0].pct, 25);
        assert_eq!(out.snapshot.metrics[0].footnote, "25 of 100");
    }

    #[tokio::test]
    async fn an_empty_scheme_sends_the_raw_key() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/usage")
            .match_header("authorization", KEY)
            .with_status(200)
            .with_body(body())
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let mut spec = spec_for(&server);
        spec.auth_scheme = String::new();
        fetch(&spec, KEY, &cache, Duration::ZERO).await.unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn a_custom_auth_header_name_and_extra_headers_are_sent() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/usage")
            .match_header("x-api-key", KEY)
            .match_header("x-org-id", "org_1")
            .match_header("authorization", mockito::Matcher::Missing)
            .with_status(200)
            .with_body(body())
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let mut spec = spec_for(&server);
        spec.auth_header = "x-api-key".into();
        spec.auth_scheme = String::new();
        spec.headers.insert("X-Org-Id".into(), "org_1".into());
        fetch(&spec, KEY, &cache, Duration::ZERO).await.unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn a_fresh_cache_short_circuits_the_network() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/usage")
            .expect(0)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache
            .write_payload(&serde_json::to_vec(&warm_snapshot()).unwrap())
            .unwrap();
        let out = fetch(&spec_for(&server), KEY, &cache, Duration::from_secs(3600))
            .await
            .unwrap();
        mock.assert_async().await;
        assert!(!out.stale);
        assert_eq!(out.snapshot, warm_snapshot());
    }

    #[tokio::test]
    async fn a_500_with_a_warm_cache_serves_the_cached_snapshot_with_the_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/usage")
            .with_status(500)
            .with_body("upstream exploded")
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache
            .write_payload(&serde_json::to_vec(&warm_snapshot()).unwrap())
            .unwrap();
        let out = fetch(&spec_for(&server), KEY, &cache, Duration::ZERO)
            .await
            .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot, warm_snapshot());
        let (code, msg) = out.last_error.expect("the 500 must be reported");
        assert_eq!(code, 500);
        assert_eq!(msg, "upstream exploded");
    }

    #[tokio::test]
    async fn a_401_with_no_cache_is_an_http_error_that_never_names_the_key() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/usage")
            .with_status(401)
            .with_body("PANCEA denied \u{1b}[31m<credential>")
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let err = fetch(&spec_for(&server), KEY, &cache, Duration::ZERO)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Http { status: 401, .. }), "{err:?}");
        let shown = err.to_string();
        assert!(!shown.contains(KEY), "{shown}");
        assert!(!shown.contains('\u{1b}'), "{shown}");
        assert!(
            !err.user_message().contains("PANCEA"),
            "{}",
            err.user_message()
        );
        assert_eq!(
            cache.read_last_error(),
            Some((401, crate::error::AUTH_FAILURE_MESSAGE.to_string()))
        );
    }

    /// The module header promises the cache holds the *projected* snapshot and
    /// not the response body, because here the schema is whatever the user
    /// pointed at — a body would park every field they did not select (an
    /// e-mail, an org id) on disk for `MAX_STALE`. That is a property of what
    /// `fetch` writes, so nothing but a test keeps a later refactor from
    /// caching the body "to save a re-parse".
    #[tokio::test]
    async fn the_cache_holds_only_the_projection_never_the_body_or_the_key() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/usage")
            .with_status(200)
            .with_body(
                r#"{"requests": {"used": 25, "limit": 100},
                    "account": {"email": "someone@example.com", "org_id": "org_1a2b"}}"#,
            )
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        fetch(&spec_for(&server), KEY, &cache, Duration::ZERO)
            .await
            .unwrap();

        let raw = String::from_utf8(std::fs::read(cache.payload_path()).unwrap()).unwrap();
        assert!(raw.contains("Requests"), "the projection is there: {raw}");
        for leaked in [
            KEY,
            "someone@example.com",
            "org_1a2b",
            "account",
            &server.url(),
        ] {
            assert!(!raw.contains(leaked), "cache leaked {leaked:?}: {raw}");
        }
    }

    #[tokio::test]
    async fn a_non_json_body_is_a_schema_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/usage")
            .with_status(200)
            .with_body("<html>sign in</html>")
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let err = fetch(&spec_for(&server), KEY, &cache, Duration::ZERO)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Schema(_)), "{err:?}");
        assert!(!err.to_string().contains("sign in"), "{err}");
    }

    #[tokio::test]
    async fn a_body_that_misses_a_pointer_is_a_schema_error_naming_the_pointer() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/usage")
            .with_status(200)
            .with_body(r#"{"requests": {"used": 1}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let err = fetch(&spec_for(&server), KEY, &cache, Duration::ZERO)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("/requests/limit is missing"),
            "{err}"
        );
    }
}
