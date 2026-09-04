//! Direct HTTPS billing fallback for Grok Build CLI versions whose ACP agent
//! no longer exposes the `x.ai/billing` extension (observed on grok 1.0.13).
//!
//! Reads the CLI's own `auth.json` and uses only its long-lived `key` entry,
//! placing it solely inside the `Authorization` header of one request to the
//! documented `cli-chat-proxy.grok.com/v1/billing` endpoint. The key is never
//! copied, cached, logged, or echoed in an error message; login files remain
//! the CLI's property and are not written back.

use std::path::Path;

use serde_json::Value;

use crate::error::{AppError, Result};
use crate::vendor::{
    HTTP_CLIENT_TIMEOUT, MAX_BODY_BYTES, read_body_capped, same_origin_redirect_policy,
};

use super::types::BillingResponse;

const DEFAULT_BASE_URL: &str = "https://cli-chat-proxy.grok.com";
const TOKEN_AUTH_HEADER: &str = "xai-grok-cli";
const MAX_AUTH_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// The base URL is fixed. Tests reach the seam through [`fetch_billing_with`],
/// so an environment override would buy nothing and would let one variable
/// redirect the login's long-lived key to a host of its choosing.
pub async fn fetch_billing(auth_path: &Path) -> Result<BillingResponse> {
    fetch_billing_with(auth_path, DEFAULT_BASE_URL).await
}

pub async fn fetch_billing_with(auth_path: &Path, base_url: &str) -> Result<BillingResponse> {
    let key = read_billing_key(auth_path)?;
    let client = reqwest::Client::builder()
        .timeout(HTTP_CLIENT_TIMEOUT)
        .redirect(same_origin_redirect_policy())
        .build()
        .map_err(|_| AppError::Other("failed to build the Grok billing HTTP client".into()))?;
    let resp = client
        .get(format!("{base_url}/v1/billing?format=credits"))
        .header("X-XAI-Token-Auth", TOKEN_AUTH_HEADER)
        .header("Authorization", format!("Bearer {key}"))
        .send()
        .await
        .map_err(|e| AppError::Transport(format!("Grok billing request failed: {e}")))?;

    let status = resp.status();
    let bytes = read_body_capped(resp, MAX_BODY_BYTES).await?;
    if !status.is_success() {
        let body: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }

    serde_json::from_slice(&bytes).map_err(|_| {
        AppError::Schema("Grok Build billing response does not match the expected schema".into())
    })
}

/// Locate the Grok Build login's long-lived `key` in `auth.json`.
///
/// The file maps issuer-prefixed client ids to login records that carry a
/// `key`. The first non-empty key wins; parsing stays bounded so a replaced
/// or oversized file cannot stall or exhaust the fetch.
pub(super) fn read_billing_key(auth_path: &Path) -> Result<String> {
    let metadata = std::fs::metadata(auth_path).map_err(|_| {
        AppError::Credentials("Grok Build login file not found; run `grok login`".into())
    })?;
    if !metadata.is_file() || metadata.len() > MAX_AUTH_FILE_BYTES {
        return Err(AppError::Credentials(
            "Grok Build login file is not a readable auth.json; run `grok login`".into(),
        ));
    }
    let bytes = std::fs::read(auth_path).map_err(|_| {
        AppError::Credentials("Grok Build login file could not be read; run `grok login`".into())
    })?;
    let parsed: Value = serde_json::from_slice(&bytes).map_err(|_| {
        AppError::Credentials("Grok Build login file is not valid JSON; run `grok login`".into())
    })?;

    let Some(entries) = parsed.as_object() else {
        return Err(AppError::Credentials(
            "Grok Build login file has an unexpected shape; run `grok login`".into(),
        ));
    };
    for entry in entries.values() {
        if let Some(key) = entry.get("key").and_then(Value::as_str) {
            let key = key.trim();
            if !key.is_empty() {
                return Ok(key.to_string());
            }
        }
    }
    Err(AppError::Credentials(
        "Grok Build login file has no billing key; run `grok login`".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn auth_file(contents: &[u8]) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(contents).unwrap();
        file
    }

    #[tokio::test]
    async fn billing_key_is_read_from_the_issuer_scoped_login_record() {
        let file = auth_file(
            br#"{"https://auth.x.ai::client-a":{"auth_mode":"oidc","key":"  secret-key  ","user_id":"person@example.test"}}"#,
        );
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/v1/billing?format=credits")
            .match_header("X-XAI-Token-Auth", "xai-grok-cli")
            .match_header("Authorization", "Bearer secret-key")
            .with_body(r#"{"config":{"creditUsagePercent":10.0,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","end":"2026-09-05T18:45:31Z"}}}"#)
            .create_async()
            .await;
        let response = fetch_billing_with(file.path(), &server.url())
            .await
            .unwrap();
        m.assert_async().await;
        drop(response);
    }

    #[tokio::test]
    async fn an_http_error_becomes_an_in_band_http_error_without_the_key() {
        let file = auth_file(br#"{"issuer::client":{"key":"secret-key"}}"#);
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/v1/billing?format=credits")
            .with_status(401)
            .with_body(r#"{"error":"bad token"}"#)
            .create_async()
            .await;
        let error = fetch_billing_with(file.path(), &server.url())
            .await
            .unwrap_err();
        m.assert_async().await;
        let rendered = error.to_string();
        assert!(!rendered.contains("secret-key"));
    }

    #[test]
    fn missing_empty_or_malformed_logins_have_actionable_errors() {
        for contents in [
            &b"{}"[..],
            br#"{"issuer::client":{"key":""}}"#,
            br#"{"issuer::client":{"refresh_token":"x"}}"#,
            b"not json",
        ] {
            let file = auth_file(contents);
            let error = read_billing_key(file.path()).unwrap_err();
            assert!(matches!(error, AppError::Credentials(_)));
            assert!(error.to_string().contains("grok login"));
        }
    }

    #[test]
    fn missing_login_file_names_the_login_step_without_echoing_the_path() {
        let error = read_billing_key(Path::new("/nonexistent/auth.json")).unwrap_err();
        assert!(matches!(error, AppError::Credentials(_)));
        assert!(!error.to_string().contains("/nonexistent"));
    }

    #[tokio::test]
    async fn oversized_login_files_fail_closed() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&vec![b'a'; MAX_AUTH_FILE_BYTES as usize + 1])
            .unwrap();
        assert!(read_billing_key(file.path()).is_err());
    }

    #[tokio::test]
    async fn oversized_billing_bodies_are_refused() {
        let file = auth_file(br#"{"issuer::client":{"key":"secret-key"}}"#);
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/v1/billing?format=credits")
            .with_body("x".repeat(MAX_BODY_BYTES + 1))
            .create_async()
            .await;
        assert!(
            fetch_billing_with(file.path(), &server.url())
                .await
                .is_err()
        );
        m.assert_async().await;
    }
    /// `GROK_CLI_CHAT_PROXY_BASE_URL` is a real Grok CLI variable — `scope.rs`
    /// hashes it into the cache digest for exactly that reason. Honouring it
    /// *here* would be different: this is the one request that carries the
    /// login's long-lived key, so an ambient variable in whatever environment
    /// Waybar inherited would choose where that key is sent. The destination
    /// stays pinned; tests reach the seam through `fetch_billing_with`.
    #[test]
    fn the_billing_destination_is_not_environment_controlled() {
        let source = include_str!("direct.rs");
        assert!(
            !crate::guard::production_code(source).contains("env::var"),
            "the credential-bearing request must not take its host from the environment"
        );
        assert_eq!(DEFAULT_BASE_URL, "https://cli-chat-proxy.grok.com");
    }
}
