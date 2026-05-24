//! Read and write `~/.claude/.credentials.json` — the OAuth state the Claude
//! CLI maintains. Mirrors claudebar:330-333 (read) and claudebar:447-452 (write).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cache::atomic_write;
use crate::error::{AppError, Result};

/// Disk shape (matches claudebar's jq paths).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    pub claude_ai_oauth: OauthCreds,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OauthCreds {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "refreshToken")]
    pub refresh_token: String,
    /// Unix epoch in **milliseconds** (claudebar:445 multiplies seconds × 1000).
    /// May arrive as a float in the wild — claudebar truncates with `%%.*`,
    /// so we accept both.
    #[serde(rename = "expiresAt", deserialize_with = "de_ms_epoch")]
    pub expires_at_ms: i64,
    #[serde(rename = "subscriptionType", default)]
    pub subscription_type: String,
    #[serde(rename = "rateLimitTier", default)]
    pub rate_limit_tier: String,
    /// Optional `scopes` array — preserved through round-trips so we don't
    /// drop information when we write back after a refresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<serde_json::Value>,
}

fn de_ms_epoch<'de, D>(d: D) -> std::result::Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Accept int or float — float values like 5000.0 are truncated.
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if let Some(f) = n.as_f64() {
                Ok(f as i64)
            } else {
                Err(serde::de::Error::custom("expiresAt not numeric"))
            }
        }
        _ => Err(serde::de::Error::custom("expiresAt must be a number")),
    }
}

impl OauthCreds {
    /// Plan label rendered the way claudebar does (claudebar:547-550):
    ///   "${sub_type^} [5x|20x]" (first letter capitalized, optional tier suffix).
    pub fn plan_label(&self) -> String {
        let mut name = capitalize_first(&self.subscription_type);
        if name.is_empty() {
            name = "Unknown".into();
        }
        if self.rate_limit_tier.contains("5x") {
            name.push_str(" 5x");
        } else if self.rate_limit_tier.contains("20x") {
            name.push_str(" 20x");
        }
        name
    }

    pub fn expires_at_secs(&self) -> i64 {
        self.expires_at_ms / 1000
    }
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => {
            let mut out = String::with_capacity(s.len());
            for c in first.to_uppercase() {
                out.push(c);
            }
            out.push_str(chars.as_str());
            out
        }
        None => String::new(),
    }
}

/// Default location: `~/.claude/.credentials.json`.
pub fn default_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| AppError::Other("HOME not set".into()))?;
    Ok(PathBuf::from(home).join(".claude/.credentials.json"))
}

pub fn read_from(path: &Path) -> Result<CredentialsFile> {
    let raw = std::fs::read_to_string(path).map_err(|e| AppError::io_at(path, e))?;
    serde_json::from_str(&raw).map_err(|e| {
        AppError::Credentials(format!(
            "could not parse {}: {e}. Run `claude` to re-authenticate.",
            path.display()
        ))
    })
}

/// Persist updated credentials, preserving any unknown top-level fields the
/// Claude CLI might have added. Reads the existing file, merges our updates
/// into the `claudeAiOauth` object, and atomically writes it back.
pub fn write_back(path: &Path, new_oauth: &OauthCreds) -> Result<()> {
    let mut doc: serde_json::Value = std::fs::read_to_string(path)
        .map_err(|e| AppError::io_at(path, e))
        .and_then(|s| serde_json::from_str(&s).map_err(AppError::Json))
        .unwrap_or_else(|_| serde_json::json!({}));

    if !doc.is_object() {
        doc = serde_json::json!({});
    }
    let obj = doc.as_object_mut().expect("just ensured object");
    obj.insert(
        "claudeAiOauth".into(),
        serde_json::to_value(new_oauth).map_err(AppError::Json)?,
    );

    let bytes = serde_json::to_vec_pretty(&doc).map_err(AppError::Json)?;
    atomic_write(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_creds(s: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(s.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn parses_canonical_shape() {
        let f = write_creds(
            r#"{"claudeAiOauth":{
                "accessToken":"AT",
                "refreshToken":"RT",
                "expiresAt": 1735000000000,
                "subscriptionType":"max",
                "rateLimitTier":"default_claude_max_5x"
            }}"#,
        );
        let creds = read_from(f.path()).unwrap();
        assert_eq!(creds.claude_ai_oauth.access_token, "AT");
        assert_eq!(creds.claude_ai_oauth.expires_at_ms, 1735000000000);
        assert_eq!(creds.claude_ai_oauth.plan_label(), "Max 5x");
    }

    #[test]
    fn accepts_float_expires_at() {
        // claudebar truncates `5000.0 → 5000`; we do the same.
        let f = write_creds(
            r#"{"claudeAiOauth":{
                "accessToken":"A","refreshToken":"R",
                "expiresAt": 5000.0,
                "subscriptionType":"pro","rateLimitTier":""
            }}"#,
        );
        let creds = read_from(f.path()).unwrap();
        assert_eq!(creds.claude_ai_oauth.expires_at_ms, 5000);
    }

    #[test]
    fn plan_label_pro_no_tier() {
        let f = write_creds(
            r#"{"claudeAiOauth":{
                "accessToken":"A","refreshToken":"R","expiresAt": 0,
                "subscriptionType":"pro","rateLimitTier":""
            }}"#,
        );
        let creds = read_from(f.path()).unwrap();
        assert_eq!(creds.claude_ai_oauth.plan_label(), "Pro");
    }

    #[test]
    fn plan_label_max_20x() {
        let f = write_creds(
            r#"{"claudeAiOauth":{
                "accessToken":"A","refreshToken":"R","expiresAt": 0,
                "subscriptionType":"max","rateLimitTier":"default_claude_max_20x"
            }}"#,
        );
        let creds = read_from(f.path()).unwrap();
        assert_eq!(creds.claude_ai_oauth.plan_label(), "Max 20x");
    }

    #[test]
    fn plan_label_empty_subscription_falls_back() {
        let f = write_creds(
            r#"{"claudeAiOauth":{
                "accessToken":"A","refreshToken":"R","expiresAt": 0,
                "subscriptionType":"","rateLimitTier":""
            }}"#,
        );
        let creds = read_from(f.path()).unwrap();
        assert_eq!(creds.claude_ai_oauth.plan_label(), "Unknown");
    }

    #[test]
    fn malformed_file_returns_credentials_error() {
        let f = write_creds("not json");
        let err = read_from(f.path()).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
    }

    #[test]
    fn write_back_round_trips_and_preserves_unknown_fields() {
        let f = write_creds(
            r#"{"claudeAiOauth":{
                "accessToken":"OLD","refreshToken":"OLD","expiresAt": 0,
                "subscriptionType":"pro","rateLimitTier":""
            },"someOtherField":"keep me"}"#,
        );
        let creds = read_from(f.path()).unwrap();
        let new_oauth = OauthCreds {
            access_token: "NEW".into(),
            refresh_token: "NEW_RT".into(),
            expires_at_ms: 1234,
            subscription_type: "pro".into(),
            rate_limit_tier: "".into(),
            scopes: creds.claude_ai_oauth.scopes.clone(),
        };
        write_back(f.path(), &new_oauth).unwrap();
        // Re-read & verify the unknown field survived.
        let raw = std::fs::read_to_string(f.path()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["someOtherField"], "keep me");
        assert_eq!(v["claudeAiOauth"]["accessToken"], "NEW");
        assert_eq!(v["claudeAiOauth"]["expiresAt"], 1234);
    }
}
