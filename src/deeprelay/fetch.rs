use std::time::Duration;

use serde::Deserialize;

use crate::cache::{Cache, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::DeepRelaySnapshot;
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

use super::types::{Balance, CachedPayload, Subscription};

pub const BASE_URL: &str = "https://api.deeprelay.ai/v1";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

pub type FetchOutcome = crate::outcome::Outcome<DeepRelaySnapshot>;

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub subscription: String,
    pub balance: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            subscription: format!("{BASE_URL}/billing/subscription"),
            balance: format!("{BASE_URL}/billing/balance"),
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
    let parse_cache = |bytes: &[u8]| -> Result<DeepRelaySnapshot> {
        let payload: CachedPayload = serde_json::from_slice(bytes)
            .map_err(|_| AppError::Schema("DeepRelay billing cache is invalid".into()))?;
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
            cache.mark_stale();
            let diagnostic = cache.write_last_error(code, &error.to_string());
            crate::outcome::fallback(cache, Some(diagnostic), error, parse_cache)
        }
    }
}

/// The wallet is only worth a second request when it can change what the user
/// does: a pay-as-you-go account *is* its balance, and a subscriber only needs
/// it once something has started billing past the plan.
fn wants_balance(subscription: &Subscription) -> bool {
    if !subscription.subscribed {
        return true;
    }
    subscription
        .usage
        .as_ref()
        .and_then(|usage| usage.recent_payg_calls_24h)
        .is_some_and(|calls| calls > 0)
}

async fn request(
    client: &reqwest::Client,
    api_key: &str,
    endpoints: &Endpoints,
) -> Result<(CachedPayload, DeepRelaySnapshot)> {
    let subscription: Subscription =
        get_json(client, api_key, &endpoints.subscription, "subscription").await?;
    // A wallet failure must leave the plan figures up: the subscription is the
    // primary reading, the balance is supplementary.
    let balance = if wants_balance(&subscription) {
        get_json::<Balance>(client, api_key, &endpoints.balance, "balance")
            .await
            .ok()
    } else {
        None
    };
    let payload = CachedPayload {
        subscription,
        balance,
    };
    let snapshot = payload.snapshot()?;
    Ok((payload, snapshot))
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
    .map_err(|_| AppError::Transport(format!("DeepRelay {what} request timed out")))??;
    let status = response.status();
    let bytes = read_body_capped(response, MAX_BODY_BYTES).await?;
    if !status.is_success() {
        return Err(http_error(status.as_u16(), &bytes));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| AppError::Schema(format!("DeepRelay {what} response schema mismatch")))
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
        // The only billing failure a user can act on: the key is valid but not
        // allowed to read the plan.
        Some("insufficient_scope") => format!(
            "{detail} — this key needs the `billing:read` scope; create one with that scope at deeprelay.ai/cloud/api-keys"
        ),
        Some("unauthenticated" | "invalid_api_key") => {
            format!("{detail} — check the key, or create a new one at deeprelay.ai/cloud/api-keys")
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
        let cache = Cache::at(dir.path().join("deeprelay"));
        cache.ensure_dir().unwrap();
        (dir, cache)
    }

    const SUBSCRIPTION: &str = include_str!("../../tests/fixtures/deeprelay/subscription.json");

    /// The cache holds the subscription *and* the wallet, so a seeded cache is
    /// the wrapped shape, not the bare response.
    fn cached_body() -> Vec<u8> {
        format!(r#"{{"subscription":{SUBSCRIPTION}}}"#).into_bytes()
    }
    const BALANCE: &str = r#"{"balance_cents":1234,"auto_pay_enabled":false,"auto_pay_threshold_cents":0,"auto_pay_amount_cents":0,"burn_cents_per_hour":0}"#;

    fn endpoints(server: &mockito::Server) -> Endpoints {
        Endpoints {
            subscription: format!("{}/billing/subscription", server.url()),
            balance: format!("{}/billing/balance", server.url()),
        }
    }

    #[tokio::test]
    async fn sends_bearer_key_and_parses_the_plan() {
        let mut server = mockito::Server::new_async().await;
        let subscription = server
            .mock("GET", "/billing/subscription")
            .match_header("authorization", "Bearer drl-test")
            .match_header("accept", "application/json")
            .with_status(200)
            .with_body(SUBSCRIPTION)
            .create_async()
            .await;
        // A subscriber with no wallet activity must not cost a second request.
        let balance = server
            .mock("GET", "/billing/balance")
            .expect(0)
            .create_async()
            .await;
        let (_dir, cache) = cache();

        let outcome = fetch_snapshot(
            &reqwest::Client::new(),
            "drl-test",
            &cache,
            &endpoints(&server),
            Duration::ZERO,
        )
        .await
        .unwrap();

        assert_eq!(outcome.snapshot.plan, "Plus");
        assert_eq!(
            outcome.snapshot.monthly_input.and_then(|m| m.pct()),
            Some(32)
        );
        assert_eq!(outcome.snapshot.weekly_window().unwrap().resets_at, None);
        assert_eq!(outcome.snapshot.balance_usd, None);
        assert!(outcome.off_the_wire());
        subscription.assert_async().await;
        balance.assert_async().await;
    }

    #[tokio::test]
    async fn reads_the_wallet_for_pay_as_you_go_accounts() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/billing/subscription")
            .with_status(200)
            .with_body(
                r#"{"subscribed":false,"plan":null,"quota":null,
                    "usage":null}"#,
            )
            .create_async()
            .await;
        server
            .mock("GET", "/billing/balance")
            .with_status(200)
            .with_body(BALANCE)
            .create_async()
            .await;
        let (_dir, cache) = cache();

        let outcome = fetch_snapshot(
            &reqwest::Client::new(),
            "drl-test",
            &cache,
            &endpoints(&server),
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(outcome.snapshot.plan, "Pay as you go");
        assert_eq!(outcome.snapshot.balance_usd, Some(12.34));
    }

    #[tokio::test]
    async fn a_failed_balance_call_keeps_the_plan_figures() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/billing/subscription")
            .with_status(200)
            .with_body(SUBSCRIPTION)
            .create_async()
            .await;
        server
            .mock("GET", "/billing/balance")
            .with_status(500)
            .with_body("{}")
            .create_async()
            .await;
        let (_dir, cache) = cache();

        let outcome = fetch_snapshot(
            &reqwest::Client::new(),
            "drl-test",
            &cache,
            &endpoints(&server),
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(outcome.snapshot.plan, "Plus");
        assert_eq!(outcome.snapshot.balance_usd, None);
        assert!(!outcome.stale);
    }

    #[tokio::test]
    async fn a_warm_cache_serves_the_previous_plan_without_a_request() {
        let mut server = mockito::Server::new_async().await;
        let request = server
            .mock("GET", "/billing/subscription")
            .expect(0)
            .create_async()
            .await;
        let (_dir, cache) = cache();
        cache.write_payload(&cached_body()).unwrap();

        let outcome = fetch_snapshot(
            &reqwest::Client::new(),
            "drl-test",
            &cache,
            &endpoints(&server),
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        assert_eq!(outcome.snapshot.plan, "Plus");
        assert!(!outcome.off_the_wire());
        assert!(!outcome.stale);
        request.assert_async().await;
    }

    #[tokio::test]
    async fn an_insufficient_scope_key_says_which_scope_to_add() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/billing/subscription")
            .with_status(403)
            .with_body(
                r#"{"type":"https://api.deeprelay.ai/errors/insufficient_scope",
                    "title":"Forbidden","status":403,
                    "detail":"This key is not allowed to read billing",
                    "code":"insufficient_scope","request_id":"abc"}"#,
            )
            .create_async()
            .await;
        let (_dir, cache) = cache();

        let error = fetch_snapshot(
            &reqwest::Client::new(),
            "drl-test",
            &cache,
            &endpoints(&server),
            Duration::ZERO,
        )
        .await
        .expect_err("scope failures have no cache to fall back on");
        assert!(matches!(
            &error,
            AppError::Http { status: 403, body } if body.contains("billing:read")
        ));
    }

    #[tokio::test]
    async fn malformed_wallet_bodies_are_a_schema_mismatch() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/billing/subscription")
            .with_status(200)
            .with_body(r#"{"error":"unauthorized"}"#)
            .create_async()
            .await;
        let (_dir, cache) = cache();

        let error = fetch_snapshot(
            &reqwest::Client::new(),
            "drl-test",
            &cache,
            &endpoints(&server),
            Duration::ZERO,
        )
        .await
        .expect_err("a body that is not a subscription must not project one");
        assert!(matches!(error, AppError::Schema(_)));
    }

    #[tokio::test]
    async fn auth_failure_keeps_the_previous_plan_stale_and_redacted() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/billing/subscription")
            .with_status(401)
            .with_body(r#"{"detail":"bad key canary","code":"unauthenticated"}"#)
            .create_async()
            .await;
        let (_dir, cache) = cache();
        cache.write_payload(&cached_body()).unwrap();

        let outcome = fetch_snapshot(
            &reqwest::Client::new(),
            "drl-test",
            &cache,
            &endpoints(&server),
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert!(outcome.stale);
        let (_, message) = outcome.last_error.expect("auth diagnostic");
        assert_eq!(message, crate::error::AUTH_FAILURE_MESSAGE);
        assert!(!message.contains("canary"));
    }

    #[tokio::test]
    async fn an_unparseable_cache_does_not_bypass_the_429_backoff() {
        let mut server = mockito::Server::new_async().await;
        let request = server
            .mock("GET", "/billing/subscription")
            .expect(0)
            .create_async()
            .await;
        let (_dir, cache) = cache();
        cache.write_payload(br#"{"unexpected":true}"#).unwrap();
        cache.write_last_error(429, "rate limited");

        let error = fetch_snapshot(
            &reqwest::Client::new(),
            "drl-test",
            &cache,
            &endpoints(&server),
            Duration::ZERO,
        )
        .await
        .expect_err("an invalid cache must not skip the backoff");
        assert!(matches!(
            error,
            AppError::Http { status: 429, body } if body.contains("next attempt in")
        ));
        request.assert_async().await;
    }
}
