//! Kiro CLI token refresh — AWS SSO OIDC (`POST https://oidc.<region>.amazonaws.com/token`,
//! documented `CreateToken`:
//! <https://docs.aws.amazon.com/singlesignon/latest/OIDCAPIReference/API_CreateToken.html>)
//! for Builder ID / IAM Identity Center sign-ins, and Kiro's desktop auth
//! service (`POST https://prod.<region>.auth.desktop.kiro.dev/refreshToken`)
//! for Google / GitHub social sign-ins.
//!
//! kiro-cli's own cached access token lives about an hour (see `db.rs`); this
//! refreshes it using the `refresh_token` (+ `client_id`/`client_secret` on the
//! OIDC path) kiro-cli already saved at `kiro-cli login` time — nothing new to
//! authenticate, just the same local credentials `db.rs` already read.
//! The refreshed token is **not** written back to kiro-cli's own database
//! (mirroring `db.rs`'s read-only treatment of that live file); `fetch.rs`
//! persists it in ai-usagebar's own account-scoped credential sidecar.

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

/// Refresh happens this far ahead of the cached expiry so a slow round-trip
/// never races the token's actual death. Mirrors `openai::oauth::REFRESH_BUFFER_SECS`.
pub const REFRESH_BUFFER_SECS: i64 = 300;

pub fn validate_region(region: &str) -> Result<()> {
    let parts: Vec<_> = region.split('-').collect();
    let valid = (3..=5).contains(&parts.len())
        && region.len() <= 32
        && parts[0].chars().all(|c| c.is_ascii_lowercase())
        && parts[parts.len() - 1].chars().all(|c| c.is_ascii_digit())
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        });
    if valid {
        Ok(())
    } else {
        Err(AppError::Credentials(
            "Kiro CLI token contains an invalid AWS region. Run `kiro-cli login` again.".into(),
        ))
    }
}

pub fn token_endpoint(region: &str) -> Result<String> {
    validate_region(region)?;
    Ok(format!("https://oidc.{region}.amazonaws.com/token"))
}

pub fn social_token_endpoint(region: &str) -> Result<String> {
    validate_region(region)?;
    Ok(format!(
        "https://prod.{region}.auth.desktop.kiro.dev/refreshToken"
    ))
}

#[derive(Debug, Serialize)]
struct RefreshRequest<'a> {
    #[serde(rename = "clientId")]
    client_id: &'a str,
    #[serde(rename = "clientSecret")]
    client_secret: &'a str,
    #[serde(rename = "grantType")]
    grant_type: &'a str,
    #[serde(rename = "refreshToken")]
    refresh_token: &'a str,
}

#[derive(Debug, Serialize)]
struct SocialRefreshRequest<'a> {
    #[serde(rename = "refreshToken")]
    refresh_token: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct RefreshResponse {
    #[serde(rename = "accessToken", deserialize_with = "de_nonempty_string")]
    pub access_token: String,
    #[serde(
        rename = "refreshToken",
        default,
        deserialize_with = "de_opt_nonempty_string"
    )]
    pub refresh_token: Option<String>,
    #[serde(rename = "expiresIn", deserialize_with = "de_positive_u64")]
    pub expires_in: u64,
    #[serde(
        rename = "profileArn",
        default,
        deserialize_with = "de_opt_profile_arn"
    )]
    pub profile_arn: Option<String>,
}

fn de_nonempty_string<'de, D>(d: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(d)?;
    if value.trim().is_empty() {
        Err(serde::de::Error::custom("accessToken cannot be empty"))
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
                Err(serde::de::Error::custom("refreshToken cannot be empty"))
            } else {
                Ok(value)
            }
        })
        .transpose()
}

fn de_opt_profile_arn<'de, D>(d: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(d)?
        .map(|value| {
            super::db::region_from_profile_arn(&value).map_err(serde::de::Error::custom)?;
            Ok(value)
        })
        .transpose()
}

fn de_positive_u64<'de, D>(d: D) -> std::result::Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Number(n) => {
            const MAX_SAFE: u64 = (i64::MAX as u64) / 2;
            if let Some(value) = n.as_u64().filter(|value| (1..=MAX_SAFE).contains(value)) {
                Ok(value)
            } else {
                Err(serde::de::Error::custom(
                    "expiresIn must be a positive integer in range",
                ))
            }
        }
        _ => Err(serde::de::Error::custom("expiresIn must be a number")),
    }
}

/// Refresh the access token against `endpoint` (build it with
/// [`token_endpoint`] for production; tests point it at mockito instead).
/// Never echoes the upstream error body verbatim — an `invalid_grant`
/// response from an OAuth token endpoint is not guaranteed not to include
/// account-identifying detail, same reasoning as `cursor::fetch::error_to_pair`.
pub async fn refresh(
    client: &reqwest::Client,
    endpoint: &str,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<RefreshResponse> {
    let req = RefreshRequest {
        client_id,
        client_secret,
        grant_type: "refresh_token",
        refresh_token,
    };
    post_refresh(client, endpoint, &req).await
}

/// Refresh a Google/GitHub social-login token against Kiro's desktop auth
/// service (`endpoint` built with [`social_token_endpoint`] in production).
pub async fn refresh_social(
    client: &reqwest::Client,
    endpoint: &str,
    refresh_token: &str,
) -> Result<RefreshResponse> {
    let req = SocialRefreshRequest { refresh_token };
    post_refresh(client, endpoint, &req).await
}

async fn post_refresh<B: Serialize + ?Sized>(
    client: &reqwest::Client,
    endpoint: &str,
    req: &B,
) -> Result<RefreshResponse> {
    let resp = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .header("User-Agent", "kiro-cli")
        .json(req)
        .send()
        .await?;

    let status = resp.status();
    let body = crate::vendor::read_body_capped(resp, crate::vendor::MAX_BODY_BYTES).await?;
    if !status.is_success() {
        return Err(AppError::Http {
            status: status.as_u16(),
            body: "Kiro CLI token refresh failed".into(),
        });
    }
    serde_json::from_slice(&body)
        .map_err(|e| AppError::Schema(format!("kiro token refresh response: {e}")))
}

pub fn needs_refresh(expires_at_secs: i64, now_secs: i64) -> bool {
    expires_at_secs < now_secs + REFRESH_BUFFER_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_endpoint_is_region_scoped() {
        assert_eq!(
            token_endpoint("us-east-1").unwrap(),
            "https://oidc.us-east-1.amazonaws.com/token"
        );
        assert_eq!(
            token_endpoint("us-gov-west-1").unwrap(),
            "https://oidc.us-gov-west-1.amazonaws.com/token"
        );
    }

    #[test]
    fn social_token_endpoint_is_region_scoped_and_rejects_unsafe_regions() {
        assert_eq!(
            social_token_endpoint("us-east-1").unwrap(),
            "https://prod.us-east-1.auth.desktop.kiro.dev/refreshToken"
        );
        for region in [
            "evil.example/#",
            "us-east-1@evil.example",
            "US-EAST-1",
            "us--1",
        ] {
            assert!(social_token_endpoint(region).is_err(), "{region}");
        }
    }

    #[test]
    fn unsafe_regions_are_rejected_before_url_construction() {
        for region in [
            "evil.example/#",
            "us-east-1@evil.example",
            "US-EAST-1",
            "us--1",
        ] {
            assert!(token_endpoint(region).is_err(), "{region}");
        }
    }

    #[test]
    fn needs_refresh_threshold() {
        let now = 1_000_000;
        assert!(needs_refresh(now + 100, now));
        assert!(!needs_refresh(now + 1000, now));
    }

    #[test]
    fn empty_access_token_is_rejected() {
        let body = r#"{"accessToken":"","expiresIn":3600}"#;
        assert!(serde_json::from_str::<RefreshResponse>(body).is_err());
    }

    #[test]
    fn malformed_expires_in_is_rejected_not_dropped() {
        for value in [r#""3600""#, "-1", "0", "null", "true"] {
            let body = format!(r#"{{"accessToken":"new","expiresIn":{value}}}"#);
            assert!(
                serde_json::from_str::<RefreshResponse>(&body).is_err(),
                "{body}"
            );
        }
        assert!(serde_json::from_str::<RefreshResponse>(r#"{"accessToken":"new"}"#).is_err());
    }

    #[test]
    fn empty_rotated_refresh_token_is_rejected() {
        let body = r#"{"accessToken":"new","refreshToken":" ","expiresIn":3600}"#;
        assert!(serde_json::from_str::<RefreshResponse>(body).is_err());
    }

    #[test]
    fn invalid_or_empty_profile_arn_in_refresh_response_is_rejected() {
        for bad_arn in [
            "",
            " ",
            "not-an-arn",
            "arn:aws:codewhisperer:evil.example/#:123:profile/ABC",
        ] {
            let body = serde_json::json!({
                "accessToken": "new",
                "expiresIn": 3600,
                "profileArn": bad_arn,
            })
            .to_string();
            assert!(
                serde_json::from_str::<RefreshResponse>(&body).is_err(),
                "{body}"
            );
        }
    }

    #[tokio::test]
    async fn refresh_success_parses_the_new_token() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/token")
            .with_status(200)
            .with_body(r#"{"accessToken":"new-at","tokenType":"Bearer","expiresIn":3600}"#)
            .create_async()
            .await;
        let client = reqwest::Client::new();
        let r = refresh(
            &client,
            &format!("{}/token", server.url()),
            "cid",
            "csecret",
            "old-rt",
        )
        .await
        .unwrap();
        assert_eq!(r.access_token, "new-at");
        assert_eq!(r.expires_in, 3600);
        assert_eq!(r.refresh_token, None);
        m.assert_async().await;
    }

    #[tokio::test]
    async fn refresh_sends_the_expected_json_body() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/token")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "clientId": "cid",
                "clientSecret": "csecret",
                "grantType": "refresh_token",
                "refreshToken": "old-rt",
            })))
            .with_status(200)
            .with_body(r#"{"accessToken":"new-at","expiresIn":3600}"#)
            .create_async()
            .await;
        let client = reqwest::Client::new();
        refresh(
            &client,
            &format!("{}/token", server.url()),
            "cid",
            "csecret",
            "old-rt",
        )
        .await
        .unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn refresh_400_does_not_echo_the_body() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/token")
            .with_status(400)
            .with_body(r#"{"error":"invalid_grant","error_description":"sensitive detail"}"#)
            .create_async()
            .await;
        let client = reqwest::Client::new();
        let err = refresh(
            &client,
            &format!("{}/token", server.url()),
            "cid",
            "csecret",
            "old-rt",
        )
        .await
        .unwrap_err();
        match err {
            AppError::Http { status, body } => {
                assert_eq!(status, 400);
                assert!(!body.contains("sensitive detail"));
            }
            other => panic!("expected Http error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn refresh_social_sends_refresh_token_and_redacts_error_body() {
        let mut server = mockito::Server::new_async().await;
        let ok_mock = server
            .mock("POST", "/refreshToken")
            .match_header("user-agent", "kiro-cli")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "refreshToken": "social-old-rt",
            })))
            .with_status(200)
            .with_body(
                r#"{"accessToken":"social-new-at","refreshToken":"social-new-rt","expiresIn":3600,"profileArn":"arn:aws:codewhisperer:us-east-1:699475941385:profile/EHGA3GRVQMUK"}"#,
            )
            .create_async()
            .await;
        let client = reqwest::Client::new();
        let r = refresh_social(
            &client,
            &format!("{}/refreshToken", server.url()),
            "social-old-rt",
        )
        .await
        .unwrap();
        assert_eq!(r.access_token, "social-new-at");
        assert_eq!(r.refresh_token.as_deref(), Some("social-new-rt"));
        assert_eq!(r.expires_in, 3600);
        assert_eq!(
            r.profile_arn.as_deref(),
            Some("arn:aws:codewhisperer:us-east-1:699475941385:profile/EHGA3GRVQMUK")
        );
        ok_mock.assert_async().await;

        let err_mock = server
            .mock("POST", "/refreshToken")
            .with_status(401)
            .with_body(r#"{"message":"sensitive social token detail"}"#)
            .create_async()
            .await;
        let err = refresh_social(
            &client,
            &format!("{}/refreshToken", server.url()),
            "social-old-rt",
        )
        .await
        .unwrap_err();
        match err {
            AppError::Http { status, body } => {
                assert_eq!(status, 401);
                assert!(!body.contains("sensitive social token detail"));
            }
            other => panic!("expected Http error, got {other:?}"),
        }
        err_mock.assert_async().await;
    }

    #[tokio::test]
    async fn refresh_does_not_follow_same_origin_or_cross_origin_redirects() {
        const CANARY_RT: &str = "KiroRefreshSecretCanary-918273";
        let client = super::super::fetch::http_client().unwrap();

        let mut server = mockito::Server::new_async().await;
        let same_origin_dest = format!("{}/redirected", server.url());
        let same_origin = server
            .mock("POST", "/refreshToken")
            .with_status(307)
            .with_header("location", &same_origin_dest)
            .create_async()
            .await;
        let redirected = server
            .mock("POST", "/redirected")
            .expect(0)
            .create_async()
            .await;

        let err = refresh_social(client, &format!("{}/refreshToken", server.url()), CANARY_RT)
            .await
            .unwrap_err();
        same_origin.assert_async().await;
        redirected.assert_async().await;
        assert!(matches!(err, AppError::Http { status: 307, .. }));
        assert!(!err.to_string().contains(CANARY_RT));

        let cross_origin = server
            .mock("POST", "/token")
            .with_status(308)
            .with_header("location", "https://attacker.invalid/collect")
            .create_async()
            .await;
        let err = refresh(
            client,
            &format!("{}/token", server.url()),
            "cid",
            "csecret",
            CANARY_RT,
        )
        .await
        .unwrap_err();
        cross_origin.assert_async().await;
        assert!(matches!(err, AppError::Http { status: 308, .. }));
        assert!(!err.to_string().contains(CANARY_RT));
    }
}
