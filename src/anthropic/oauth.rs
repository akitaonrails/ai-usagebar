//! OAuth token refresh — POST `https://platform.claude.com/v1/oauth/token`.
//!
//! Mirrors claudebar:425-489. Notable details:
//! - `client_id` is the public Claude CLI ID (not a secret).
//! - Beta header `anthropic-beta: oauth-2025-04-20` is required.
//! - `User-Agent: claude-cli/1.0` matches the CLI; some endpoints gate on it.
//! - `expires_in` may arrive as float; we accept either.
//! - The server sometimes returns a rotated `refresh_token`, sometimes not —
//!   callers fall back to the old one when absent.

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

pub const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const BETA_HEADER: &str = "oauth-2025-04-20";
pub const USER_AGENT: &str = "claude-cli/1.0";

/// Refresh-window — refresh if the token expires within this many seconds.
/// claudebar:99 `REFRESH_BUFFER=300`.
pub const REFRESH_BUFFER_SECS: i64 = 300;

#[derive(Debug, Serialize)]
struct RefreshRequest<'a> {
    grant_type: &'a str,
    client_id: &'a str,
    refresh_token: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct RefreshResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(deserialize_with = "de_expires_in")]
    pub expires_in: u64,
}

fn de_expires_in<'de, D>(d: D) -> std::result::Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                Ok(u)
            } else if let Some(f) = n.as_f64() {
                Ok(f as u64)
            } else {
                Err(serde::de::Error::custom("expires_in not numeric"))
            }
        }
        _ => Err(serde::de::Error::custom("expires_in must be a number")),
    }
}

/// Try to refresh the access token. The `endpoint` arg is parameterized for
/// tests (defaults to [`TOKEN_URL`] in production).
pub async fn refresh(
    client: &reqwest::Client,
    endpoint: &str,
    refresh_token: &str,
) -> Result<RefreshResponse> {
    let req = RefreshRequest {
        grant_type: "refresh_token",
        client_id: CLIENT_ID,
        refresh_token,
    };

    let resp = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .header("anthropic-beta", BETA_HEADER)
        .header("User-Agent", USER_AGENT)
        .json(&req)
        .send()
        .await?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        // claudebar tolerates three error-body shapes (claudebar:464-476):
        //   {error_description: "..."}, {error: {message: "..."}}, {error: "..."}.
        let msg = parse_error_body(&body).unwrap_or_else(|| {
            if status.as_u16() < 500 {
                "Refresh failed".into()
            } else {
                "Invalid refresh response".into()
            }
        });
        return Err(AppError::Http {
            status: status.as_u16(),
            body: msg,
        });
    }

    serde_json::from_str(&body)
        .map_err(|e| AppError::Schema(format!("token refresh response: {e}; body: {body}")))
}

/// Extract a human-readable error message from a non-2xx refresh body.
///
/// Tolerates three shapes claudebar handles at lines 467-472.
pub fn parse_error_body(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    if let Some(s) = v.get("error_description").and_then(|x| x.as_str()) {
        return Some(s.to_string());
    }
    if let Some(s) = v
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|x| x.as_str())
    {
        return Some(s.to_string());
    }
    if let Some(s) = v.get("error").and_then(|x| x.as_str()) {
        return Some(s.to_string());
    }
    None
}

/// True when the token expires within the refresh buffer (or has already).
pub fn needs_refresh(expires_at_secs: i64, now_secs: i64) -> bool {
    expires_at_secs < now_secs + REFRESH_BUFFER_SECS
}

/// True when we actually hold a refresh token to refresh *with*. (See the
/// caller in `fetch_snapshot` for why an empty one must skip the refresh.)
pub fn can_refresh(refresh_token: &str) -> bool {
    !refresh_token.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_refresh_when_within_buffer() {
        let now = 1_000_000;
        // Expires in 100s → within 300s buffer → refresh.
        assert!(needs_refresh(now + 100, now));
        // Expires in 1000s → outside buffer → no refresh.
        assert!(!needs_refresh(now + 1000, now));
        // Already expired → refresh.
        assert!(needs_refresh(now - 1, now));
    }

    #[test]
    fn can_refresh_false_for_empty_or_blank_token() {
        assert!(!can_refresh(""));
        assert!(!can_refresh("   "));
        assert!(can_refresh("sk-ant-ort01-real-token"));
    }

    #[test]
    fn parse_error_body_oauth_style() {
        let s = r#"{"error":"invalid_grant","error_description":"Refresh token expired"}"#;
        assert_eq!(
            parse_error_body(s).as_deref(),
            Some("Refresh token expired")
        );
    }

    #[test]
    fn parse_error_body_anthropic_object() {
        let s = r#"{"error":{"type":"authentication_error","message":"Token invalid"}}"#;
        assert_eq!(parse_error_body(s).as_deref(), Some("Token invalid"));
    }

    #[test]
    fn parse_error_body_bare_string() {
        let s = r#"{"error":"Something went wrong"}"#;
        assert_eq!(parse_error_body(s).as_deref(), Some("Something went wrong"));
    }

    #[test]
    fn parse_error_body_unrecognized_shape_returns_none() {
        let s = r#"{"unknown":"shape"}"#;
        assert!(parse_error_body(s).is_none());
    }

    #[test]
    fn parse_error_body_invalid_json_returns_none() {
        assert!(parse_error_body("not json").is_none());
    }

    #[tokio::test]
    async fn refresh_success_parses_response() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/v1/oauth/token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"new-at","refresh_token":"new-rt","expires_in":3600}"#)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let resp = refresh(
            &client,
            &format!("{}/v1/oauth/token", server.url()),
            "old-rt",
        )
        .await
        .unwrap();
        assert_eq!(resp.access_token, "new-at");
        assert_eq!(resp.refresh_token.as_deref(), Some("new-rt"));
        assert_eq!(resp.expires_in, 3600);
        m.assert_async().await;
    }

    #[tokio::test]
    async fn refresh_accepts_float_expires_in() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/v1/oauth/token")
            .with_status(200)
            .with_body(r#"{"access_token":"new","expires_in":3600.0}"#)
            .create_async()
            .await;
        let client = reqwest::Client::new();
        let resp = refresh(&client, &format!("{}/v1/oauth/token", server.url()), "x")
            .await
            .unwrap();
        assert_eq!(resp.expires_in, 3600);
        assert!(resp.refresh_token.is_none());
    }

    #[tokio::test]
    async fn refresh_400_with_oauth_error_returns_http_with_description() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/v1/oauth/token")
            .with_status(400)
            .with_body(r#"{"error":"invalid_grant","error_description":"Refresh token expired"}"#)
            .create_async()
            .await;
        let client = reqwest::Client::new();
        let err = refresh(&client, &format!("{}/v1/oauth/token", server.url()), "x")
            .await
            .unwrap_err();
        match err {
            AppError::Http { status, body } => {
                assert_eq!(status, 400);
                assert_eq!(body, "Refresh token expired");
            }
            other => panic!("expected Http error, got {other:?}"),
        }
    }
}
