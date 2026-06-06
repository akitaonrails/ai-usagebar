//! Read and write `~/.claude/.credentials.json` — the OAuth state the Claude
//! CLI maintains. Mirrors claudebar:330-333 (read) and claudebar:447-452 (write).
//!
//! On macOS the file often doesn't exist: recent Claude Code builds keep the
//! same JSON in the login Keychain instead. When the file is absent we transom
//! over to [`keychain`] so subscription usage works there too.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cache::atomic_write;
use crate::error::{AppError, Result};

#[cfg(target_os = "macos")]
use super::keychain;

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

/// Default location: `~/.claude/.credentials.json` (Unix/macOS) or
/// `%USERPROFILE%\.claude\.credentials.json` (Windows).
///
/// Home is resolved through [`crate::cache::home_dir`] so every platform's
/// convention is honored in one place.
pub fn default_path() -> Result<PathBuf> {
    Ok(crate::cache::home_dir()?
        .join(".claude")
        .join(".credentials.json"))
}

pub fn read_from(path: &Path) -> Result<CredentialsFile> {
    match std::fs::read_to_string(path) {
        Ok(raw) => parse(&raw, &path.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No file — on macOS the credentials usually live in the Keychain.
            #[cfg(target_os = "macos")]
            if let Some(raw) = keychain::read_raw()? {
                return parse(&raw, "macOS Keychain (Claude Code-credentials)");
            }
            Err(AppError::io_at(path, e))
        }
        Err(e) => Err(AppError::io_at(path, e)),
    }
}

/// Parse a credentials JSON blob from any source (`source` only labels errors).
fn parse(raw: &str, source: &str) -> Result<CredentialsFile> {
    serde_json::from_str(raw).map_err(|e| {
        AppError::Credentials(format!(
            "could not parse {source}: {e}. Run `claude` to re-authenticate."
        ))
    })
}

/// Merge a refreshed `claudeAiOauth` into an existing credentials document
/// (or a fresh `{}`), preserving any unknown top-level fields the Claude CLI
/// keeps there (e.g. `mcpOAuth`). Pure so the merge is unit-testable without
/// touching disk or the Keychain.
fn merge_oauth(existing: Option<&str>, new_oauth: &OauthCreds) -> Result<serde_json::Value> {
    let mut doc: serde_json::Value = existing
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !doc.is_object() {
        doc = serde_json::json!({});
    }
    doc.as_object_mut().expect("just ensured object").insert(
        "claudeAiOauth".into(),
        serde_json::to_value(new_oauth).map_err(AppError::Json)?,
    );
    Ok(doc)
}

/// Persist updated credentials, preserving any unknown top-level fields the
/// Claude CLI might have added. Writes back to wherever the creds actually
/// live: the file if present, otherwise (macOS) the Keychain item — keeping a
/// single shared source of truth with Claude Code instead of forking a stale
/// copy and rotating the refresh token out from under it.
pub fn write_back(path: &Path, new_oauth: &OauthCreds) -> Result<()> {
    #[cfg(target_os = "macos")]
    if !path.exists() {
        if let Some(existing) = keychain::read_raw()? {
            let doc = merge_oauth(Some(&existing), new_oauth)?;
            let json = serde_json::to_string(&doc).map_err(AppError::Json)?;
            return keychain::write_raw(&json);
        }
    }

    let existing = std::fs::read_to_string(path).ok();
    let doc = merge_oauth(existing.as_deref(), new_oauth)?;
    let bytes = serde_json::to_vec_pretty(&doc).map_err(AppError::Json)?;
    atomic_write(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    fn write_creds(s: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(s.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    /// Like `write_creds`, but writes to a named file inside a `TempDir` and
    /// closes the handle. `write_back` rewrites the file via an atomic
    /// rename-over-destination, which on Windows fails if the destination is
    /// still open (as a live `NamedTempFile` handle would be). Returns the dir
    /// (kept alive by the caller) and the closed file's path.
    fn write_creds_closed(s: &str) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(&path, s).unwrap();
        (dir, path)
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

    // Linux-only: a missing file with no Keychain fallback is an I/O error,
    // not a parse error. Gated off macOS so we never read the developer's real
    // Keychain item (which would both flake and surface a live token).
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn read_from_missing_file_is_io_error() {
        let path = std::path::Path::new("/nonexistent/ai-usagebar/.credentials.json");
        let err = read_from(path).unwrap_err();
        assert!(matches!(err, AppError::Io { .. }));
    }

    #[test]
    fn default_path_ends_with_claude_credentials() {
        let p = default_path().unwrap();
        // The trailing two segments are stable across platforms; only the home
        // prefix differs (resolved by directories::BaseDirs).
        assert!(p.ends_with(std::path::Path::new(".claude").join(".credentials.json")));
    }

    // On Windows the home prefix is %USERPROFILE%, not $HOME — assert the
    // resolver honors it so the credential file is found natively.
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

    #[test]
    fn merge_oauth_preserves_unknown_top_level_fields() {
        let existing = r#"{"claudeAiOauth":{"accessToken":"OLD"},"mcpOAuth":{"x":1}}"#;
        let new_oauth = OauthCreds {
            access_token: "NEW".into(),
            refresh_token: "RT".into(),
            expires_at_ms: 99,
            subscription_type: "max".into(),
            rate_limit_tier: "".into(),
            scopes: None,
        };
        let doc = merge_oauth(Some(existing), &new_oauth).unwrap();
        assert_eq!(doc["mcpOAuth"]["x"], 1);
        assert_eq!(doc["claudeAiOauth"]["accessToken"], "NEW");
        assert_eq!(doc["claudeAiOauth"]["expiresAt"], 99);
    }

    #[test]
    fn merge_oauth_handles_empty_and_non_object_input() {
        let new_oauth = OauthCreds {
            access_token: "A".into(),
            refresh_token: "R".into(),
            expires_at_ms: 0,
            subscription_type: "pro".into(),
            rate_limit_tier: "".into(),
            scopes: None,
        };
        // None → fresh object.
        let doc = merge_oauth(None, &new_oauth).unwrap();
        assert_eq!(doc["claudeAiOauth"]["accessToken"], "A");
        // Garbage / non-object → discarded, fresh object.
        let doc = merge_oauth(Some("not json"), &new_oauth).unwrap();
        assert_eq!(doc["claudeAiOauth"]["accessToken"], "A");
        let doc = merge_oauth(Some("[1,2,3]"), &new_oauth).unwrap();
        assert_eq!(doc["claudeAiOauth"]["accessToken"], "A");
    }

    #[test]
    fn write_back_round_trips_and_preserves_unknown_fields() {
        let (_dir, path) = write_creds_closed(
            r#"{"claudeAiOauth":{
                "accessToken":"OLD","refreshToken":"OLD","expiresAt": 0,
                "subscriptionType":"pro","rateLimitTier":""
            },"someOtherField":"keep me"}"#,
        );
        let creds = read_from(&path).unwrap();
        let new_oauth = OauthCreds {
            access_token: "NEW".into(),
            refresh_token: "NEW_RT".into(),
            expires_at_ms: 1234,
            subscription_type: "pro".into(),
            rate_limit_tier: "".into(),
            scopes: creds.claude_ai_oauth.scopes.clone(),
        };
        write_back(&path, &new_oauth).unwrap();
        // Re-read & verify the unknown field survived.
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["someOtherField"], "keep me");
        assert_eq!(v["claudeAiOauth"]["accessToken"], "NEW");
        assert_eq!(v["claudeAiOauth"]["expiresAt"], 1234);
    }
}
