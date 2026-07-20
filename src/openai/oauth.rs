//! OAuth refresh — POST `https://auth.openai.com/oauth/token`.
//!
//! Mirrors codexbar's refresh flow and `openai/codex`'s `auth/manager.rs`.
//! Notable differences from the Anthropic flow:
//!   - URL is `auth.openai.com` (not `platform.claude.com`)
//!   - `client_id` is the Codex CLI's public OAuth client ID
//!   - The body must include `scope: "openid profile email"`
//!   - The response includes a fresh `id_token` too (we persist all three).

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

pub const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const SCOPE: &str = "openid profile email";
pub const REFRESH_BUFFER_SECS: i64 = 300;

#[derive(Debug, Serialize)]
struct RefreshRequest<'a> {
    client_id: &'a str,
    grant_type: &'a str,
    refresh_token: &'a str,
    scope: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct RefreshResponse {
    #[serde(deserialize_with = "de_nonempty_string")]
    pub access_token: String,
    #[serde(default, deserialize_with = "de_opt_nonempty_string")]
    pub refresh_token: Option<String>,
    #[serde(default, deserialize_with = "de_opt_nonempty_string")]
    pub id_token: Option<String>,
    #[serde(default, deserialize_with = "de_expires_in")]
    pub expires_in: Option<u64>,
}

fn de_nonempty_string<'de, D>(d: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(d)?;
    if value.trim().is_empty() {
        Err(serde::de::Error::custom("token cannot be empty"))
    } else {
        Ok(value)
    }
}

fn de_opt_nonempty_string<'de, D>(d: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(d)?
        .map(|value| {
            if value.trim().is_empty() {
                Err(serde::de::Error::custom("token cannot be empty"))
            } else {
                Ok(value)
            }
        })
        .transpose()
}

fn de_expires_in<'de, D>(d: D) -> std::result::Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(n) => {
            const MAX_SAFE_EXPIRES_IN: u64 = (i64::MAX as u64) / 2;
            if let Some(value) = n.as_u64().filter(|value| *value <= MAX_SAFE_EXPIRES_IN) {
                Ok(Some(value))
            } else if let Some(value) = n.as_f64()
                && value.is_finite()
                && value.fract() == 0.0
                && (0.0..=MAX_SAFE_EXPIRES_IN as f64).contains(&value)
            {
                Ok(Some(value as u64))
            } else {
                Err(serde::de::Error::custom(
                    "expires_in must be a non-negative integer in range",
                ))
            }
        }
        other => Err(serde::de::Error::custom(format!(
            "expires_in must be a number or null, got {other:?}"
        ))),
    }
}

pub async fn refresh(
    client: &reqwest::Client,
    endpoint: &str,
    refresh_token: &str,
) -> Result<RefreshResponse> {
    let req = RefreshRequest {
        client_id: CLIENT_ID,
        grant_type: "refresh_token",
        refresh_token,
        scope: SCOPE,
    };

    let resp = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .json(&req)
        .send()
        .await?;

    let status = resp.status();
    let body = crate::vendor::read_body_capped(resp, crate::vendor::MAX_BODY_BYTES).await?;
    let body = String::from_utf8_lossy(&body).into_owned();
    if !status.is_success() {
        let msg = crate::anthropic::oauth::parse_error_body(&body)
            .unwrap_or_else(|| "Refresh failed".into());
        return Err(AppError::Http {
            status: status.as_u16(),
            body: msg,
        });
    }
    serde_json::from_str(&body)
        .map_err(|e| AppError::Schema(format!("openai token response: {e}; body: {body}")))
}

pub fn needs_refresh(expires_at_secs: i64, now_secs: i64) -> bool {
    expires_at_secs < now_secs + REFRESH_BUFFER_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_refresh_threshold() {
        let now = 1_000_000;
        assert!(needs_refresh(now + 100, now));
        assert!(!needs_refresh(now + 1000, now));
    }

    #[test]
    fn malformed_optional_expires_in_is_not_treated_as_absent() {
        for value in [
            "3600.5",
            "-1",
            "1e300",
            "18446744073709551615",
            "true",
            r#""3600""#,
        ] {
            let body = format!(r#"{{"access_token":"new","expires_in":{value}}}"#);
            assert!(
                serde_json::from_str::<RefreshResponse>(&body).is_err(),
                "{body}"
            );
        }
        let response: RefreshResponse =
            serde_json::from_str(r#"{"access_token":"new","expires_in":null}"#).unwrap();
        assert_eq!(response.expires_in, None);
    }

    #[test]
    fn empty_refresh_tokens_are_schema_drift_not_credentials_to_persist() {
        for body in [
            r#"{"access_token":""}"#,
            r#"{"access_token":"new","refresh_token":"   "}"#,
            r#"{"access_token":"new","id_token":""}"#,
        ] {
            assert!(
                serde_json::from_str::<RefreshResponse>(body).is_err(),
                "{body}"
            );
        }
    }

    #[tokio::test]
    async fn refresh_success_parses_three_tokens() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/oauth/token")
            .with_status(200)
            .with_body(
                r#"{"access_token":"new-at","refresh_token":"new-rt","id_token":"new-id","expires_in":3600}"#,
            )
            .create_async()
            .await;
        let client = reqwest::Client::new();
        let r = refresh(&client, &format!("{}/oauth/token", server.url()), "old")
            .await
            .unwrap();
        assert_eq!(r.access_token, "new-at");
        assert_eq!(r.refresh_token.as_deref(), Some("new-rt"));
        assert_eq!(r.id_token.as_deref(), Some("new-id"));
        assert_eq!(r.expires_in, Some(3600));
    }

    #[tokio::test]
    async fn refresh_400_returns_http_with_description() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/oauth/token")
            .with_status(400)
            .with_body(r#"{"error":"invalid_grant","error_description":"Refresh expired"}"#)
            .create_async()
            .await;
        let client = reqwest::Client::new();
        let err = refresh(&client, &format!("{}/oauth/token", server.url()), "x")
            .await
            .unwrap_err();
        match err {
            AppError::Http { status, body } => {
                assert_eq!(status, 400);
                assert_eq!(body, "Refresh expired");
            }
            other => panic!("expected Http error, got {other:?}"),
        }
    }
}
