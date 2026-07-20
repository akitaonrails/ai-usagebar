//! Read and write `~/.codex/auth.json` — the OAuth state the OpenAI Codex CLI
//! maintains. Mirrors codexbar's jq paths.

use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::cache::atomic_write;
use crate::error::{AppError, Result};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthFile {
    pub tokens: Tokens,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_refresh: Option<String>,
    #[serde(flatten, default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: String,
    #[serde(default)]
    pub account_id: Option<String>,
    /// Optional explicit expiry from the OAuth server. When absent, we infer
    /// from the id_token's `exp` claim.
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(flatten, default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Default location: `~/.codex/auth.json` (Unix/macOS) or
/// `%USERPROFILE%\.codex\auth.json` (Windows).
///
/// Home is resolved through [`crate::cache::home_dir`] so every platform's
/// convention is honored in one place.
pub fn default_path() -> Result<PathBuf> {
    Ok(crate::cache::home_dir()?.join(".codex").join("auth.json"))
}

pub fn read_from(path: &Path) -> Result<AuthFile> {
    let raw = std::fs::read_to_string(path).map_err(|e| AppError::io_at(path, e))?;
    serde_json::from_str(&raw).map_err(|e| {
        AppError::Credentials(format!(
            "could not parse {}: {e}. Run `codex login` to re-authenticate.",
            path.display()
        ))
    })
}

/// Persist updated tokens, preserving any unknown fields. Atomic.
pub fn write_back(path: &Path, auth: &AuthFile) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(auth).map_err(AppError::Json)?;
    atomic_write(path, &bytes)
}

impl Tokens {
    /// Compute the Unix-seconds expiry. A persisted `expires_at` is the newest
    /// information after a refresh and must win over the id_token's claim: the
    /// token endpoint may return `expires_in` without returning a replacement
    /// id_token, leaving the old (expired) claim in place. Fall back to the
    /// access-token JWT (the source current Codex itself uses), then the legacy
    /// id-token claim, for auth files that do not carry the explicit field.
    /// Returns 0 (forcing an immediate refresh) when neither is usable.
    pub fn expires_at_secs(&self) -> i64 {
        self.expires_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp())
            .or_else(|| parse_jwt_exp(&self.access_token))
            .or_else(|| parse_jwt_exp(&self.id_token))
            .unwrap_or(0)
    }

    /// Plan tier from the id_token's nested claim
    /// `https://api.openai.com/auth.chatgpt_plan_type`.
    pub fn plan_type_from_id_token(&self) -> Option<String> {
        let claims = parse_jwt_claims(&self.id_token)?;
        claims
            .get("https://api.openai.com/auth")
            .and_then(|v| v.get("chatgpt_plan_type"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }
}

/// Parse a JWT's `exp` claim. Returns None for malformed tokens.
fn parse_jwt_exp(token: &str) -> Option<i64> {
    let claims = parse_jwt_claims(token)?;
    claims
        .get("exp")
        .and_then(|v| v.as_i64())
        .or_else(|| claims.get("exp").and_then(|v| v.as_f64()).map(|f| f as i64))
}

fn parse_jwt_claims(token: &str) -> Option<serde_json::Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    fn write_auth(s: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(s.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    /// Like `write_auth`, but with no open handle on the file, so
    /// `write_back`'s atomic rename-over-destination succeeds on Windows.
    /// See [`crate::cache::closed_temp_file`].
    fn write_auth_closed(s: &str) -> (TempDir, std::path::PathBuf) {
        crate::cache::closed_temp_file("auth.json", Some(s))
    }

    /// Build a fake JWT with the given claims (no signature verification).
    fn fake_jwt(claims: serde_json::Value) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"alg":"none","typ":"JWT"}"#);
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn parses_minimal_auth_file() {
        let jwt = fake_jwt(serde_json::json!({"exp": 1234567890}));
        let body = format!(
            r#"{{"tokens":{{"access_token":"AT","refresh_token":"RT",
                "id_token":"{jwt}","account_id":"acc"}}}}"#
        );
        let f = write_auth(&body);
        let auth = read_from(f.path()).unwrap();
        assert_eq!(auth.tokens.access_token, "AT");
        assert_eq!(auth.tokens.account_id.as_deref(), Some("acc"));
        assert_eq!(auth.tokens.expires_at_secs(), 1234567890);
    }

    #[test]
    fn extracts_plan_type_from_id_token() {
        let jwt = fake_jwt(serde_json::json!({
            "exp": 1234567890,
            "https://api.openai.com/auth": {"chatgpt_plan_type": "plus"}
        }));
        let body = format!(
            r#"{{"tokens":{{"access_token":"AT","refresh_token":"RT","id_token":"{jwt}"}}}}"#
        );
        let f = write_auth(&body);
        let auth = read_from(f.path()).unwrap();
        assert_eq!(
            auth.tokens.plan_type_from_id_token().as_deref(),
            Some("plus")
        );
    }

    #[test]
    fn malformed_jwt_returns_zero_exp() {
        let body = r#"{"tokens":{"access_token":"x","refresh_token":"y","id_token":"not.a.jwt"}}"#;
        let f = write_auth(body);
        let auth = read_from(f.path()).unwrap();
        assert_eq!(auth.tokens.expires_at_secs(), 0);
        assert!(auth.tokens.plan_type_from_id_token().is_none());
    }

    #[test]
    fn explicit_expires_at_overrides_an_old_expired_id_token() {
        // A refresh that returns no new id_token used to leave the old expired
        // claim in place. The refreshed `expires_at` must win or every later
        // run refreshes again.
        let expired_jwt = fake_jwt(serde_json::json!({"exp": 1}));
        let mut tokens = Tokens {
            access_token: "AT".into(),
            refresh_token: "RT".into(),
            id_token: expired_jwt,
            account_id: None,
            expires_at: Some("2030-01-01T00:00:00Z".into()),
            extra: Default::default(),
        };
        let expected = chrono::DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(tokens.expires_at_secs(), expected);

        // An invalid explicit value still falls back to the JWT.
        tokens.expires_at = Some("whenever".into());
        assert_eq!(tokens.expires_at_secs(), 1);
    }

    #[test]
    fn access_token_expiry_wins_over_the_legacy_id_token_claim() {
        let tokens = Tokens {
            access_token: fake_jwt(serde_json::json!({"exp": 2_000_000_000})),
            refresh_token: "RT".into(),
            id_token: fake_jwt(serde_json::json!({"exp": 1})),
            account_id: None,
            expires_at: None,
            extra: Default::default(),
        };
        assert_eq!(tokens.expires_at_secs(), 2_000_000_000);
    }

    #[test]
    fn malformed_file_returns_credentials_error() {
        let f = write_auth("not json");
        let err = read_from(f.path()).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
    }

    #[test]
    fn write_back_preserves_unknown_fields() {
        let jwt = fake_jwt(serde_json::json!({"exp": 1234567890}));
        let body = format!(
            r#"{{"tokens":{{"access_token":"AT","refresh_token":"RT","id_token":"{jwt}"}},
                "some_other_field":"keep-me"}}"#
        );
        let (_dir, path) = write_auth_closed(&body);
        let mut auth = read_from(&path).unwrap();
        auth.tokens.access_token = "NEW".into();
        write_back(&path, &auth).unwrap();

        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["some_other_field"], "keep-me");
        assert_eq!(v["tokens"]["access_token"], "NEW");
    }

    #[test]
    fn default_path_ends_with_codex_auth() {
        let p = default_path().unwrap();
        // Trailing segments are stable across platforms; only the home prefix
        // differs (resolved by directories::BaseDirs).
        assert!(p.ends_with(std::path::Path::new(".codex").join("auth.json")));
    }

    // On Windows the home prefix is %USERPROFILE%, not $HOME.
    #[cfg(windows)]
    #[test]
    fn default_path_uses_userprofile_on_windows() {
        let p = default_path().unwrap();
        let userprofile = std::env::var("USERPROFILE").expect("USERPROFILE set on Windows");
        // directories::BaseDirs resolves the home via SHGetKnownFolderPath, which
        // can differ from %USERPROFILE% in casing or path separator. Compare on a
        // normalized basis (lowercased, backslashes) rather than Path::starts_with,
        // which compares components case-sensitively even on Windows.
        let norm = |s: &str| s.to_lowercase().replace('/', "\\");
        let p_norm = norm(&p.to_string_lossy());
        let up_norm = norm(&userprofile);
        assert!(
            p_norm.starts_with(up_norm.as_str()),
            "{} should live under {}",
            p.display(),
            userprofile
        );
    }
}
