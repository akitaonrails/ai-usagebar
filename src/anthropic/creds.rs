//! Read and write `~/.claude/.credentials.json` — the OAuth state the Claude
//! CLI maintains. Mirrors claudebar:330-333 (read) and claudebar:447-452 (write).
//!
//! On macOS the file often doesn't exist: recent Claude Code builds keep the
//! same JSON in the login Keychain instead. For the *default* location we fall
//! back to [`keychain`] when the file is missing or clearly unusable (#15);
//! explicit paths (`--creds-path`, config, named accounts) are read strictly.

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

/// Strict file read: no Keychain fallback, ever. Explicit paths (`--creds-path`,
/// config `credentials_path`, named accounts) go through here so a missing or
/// broken file fails loudly instead of silently reading a *different* account's
/// credentials from the Keychain (issues #14/#15).
pub fn read_from(path: &Path) -> Result<CredentialsFile> {
    match std::fs::read_to_string(path) {
        Ok(raw) => parse(&raw, &path.display().to_string()),
        Err(e) => Err(AppError::io_at(path, e)),
    }
}

/// Which credentials location a fetch should use. Only the platform-default
/// location is eligible for the macOS Keychain fallback — Claude Code owns
/// that location, so "look where Claude Code lives" includes its Keychain
/// item. An explicit path is a user decision and is honored strictly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredsTarget {
    /// `~/.claude/.credentials.json` (or the Windows equivalent) — falls back
    /// to the macOS Keychain when the file is missing *or unusable* (#15).
    Default(PathBuf),
    /// `--creds-path`, config `credentials_path`, or a named account's file —
    /// never consults the Keychain.
    Explicit(PathBuf),
}

impl CredsTarget {
    pub fn path(&self) -> &Path {
        match self {
            CredsTarget::Default(p) | CredsTarget::Explicit(p) => p,
        }
    }
}

/// Where credentials were actually read from. Write-backs must follow this —
/// refreshing tokens read from the Keychain into a stale file (or vice versa)
/// would fork the credential state Claude Code depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredsSource {
    File(PathBuf),
    /// Only produced on macOS in production (the login Keychain item
    /// `Claude Code-credentials`).
    Keychain,
}

/// Credentials that can't possibly authenticate anything: no access token, no
/// refresh token, no future expiry. This is the #15 predicate — deliberately
/// narrow so the v0.7.2 trusted-device shape (empty `refreshToken` but a live
/// `accessToken`) keeps its file-first behavior.
pub fn is_unusable(oauth: &OauthCreds) -> bool {
    oauth.access_token.trim().is_empty()
        && oauth.refresh_token.trim().is_empty()
        && oauth.expires_at_ms <= 0
}

/// Resolve a [`CredsTarget`] to actual credentials + the source they came
/// from. `Explicit` is a strict [`read_from`]; `Default` adds the macOS
/// Keychain fallback.
pub fn resolve(target: &CredsTarget) -> Result<(CredentialsFile, CredsSource)> {
    match target {
        CredsTarget::Explicit(p) => Ok((read_from(p)?, CredsSource::File(p.clone()))),
        CredsTarget::Default(p) => {
            #[cfg(target_os = "macos")]
            return read_default_with(p, keychain::read_raw);
            #[cfg(not(target_os = "macos"))]
            read_default_with(p, || Ok(None))
        }
    }
}

/// Default-location read with an injectable Keychain reader, so the fallback
/// selection is unit-testable on any platform (the hermeticity invariant —
/// tests must never touch a real Keychain). Decision table:
///
/// | file state          | keychain        | outcome                    |
/// |---------------------|-----------------|----------------------------|
/// | usable              | (not consulted*)| file                       |
/// | missing             | usable          | keychain                   |
/// | missing             | absent          | original I/O error         |
/// | unusable (#15)      | usable          | keychain                   |
/// | unusable (#15)      | absent          | file result, unchanged     |
/// | unparsable JSON     | usable          | keychain                   |
/// | unparsable JSON     | absent          | original parse error       |
/// | any                 | **unreadable**  | the Keychain error         |
///
/// The last row matters: a *locked* login Keychain or a denied ACL is not the
/// same as "no credentials". Reporting it as absent surfaced a "run `claude`"
/// message and sent users to re-authenticate while their credentials were
/// sitting there intact, so that failure now wins over the file's own error.
///
/// *usable file short-circuits — no `security(1)` subprocess on the happy path.
fn read_default_with(
    path: &Path,
    keychain_read: impl Fn() -> Result<Option<String>>,
) -> Result<(CredentialsFile, CredsSource)> {
    let file_result = read_from(path);
    match &file_result {
        Ok(creds) if !is_unusable(&creds.claude_ai_oauth) => {
            Ok((file_result?, CredsSource::File(path.to_path_buf())))
        }
        // Missing, unusable, or unparsable — see if the Keychain has better.
        _ => match keychain_read()? {
            Some(raw) => match parse(&raw, "macOS Keychain (Claude Code-credentials)") {
                Ok(kc) if !is_unusable(&kc.claude_ai_oauth) => Ok((kc, CredsSource::Keychain)),
                // Keychain no better than the file — surface the file outcome.
                _ => Ok((file_result?, CredsSource::File(path.to_path_buf()))),
            },
            None => Ok((file_result?, CredsSource::File(path.to_path_buf()))),
        },
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

/// Persist updated credentials to the source they were actually read from
/// (see [`CredsSource`]), preserving any unknown top-level fields the Claude
/// CLI might have added. Following the read source keeps a single shared
/// source of truth with Claude Code — refreshing Keychain-read tokens into a
/// stale shadow file would rotate the refresh token out from under it.
pub fn write_back_to(source: &CredsSource, new_oauth: &OauthCreds) -> Result<()> {
    match source {
        CredsSource::File(path) => write_back(path, new_oauth),
        #[cfg(target_os = "macos")]
        CredsSource::Keychain => {
            let existing = keychain::read_raw()?;
            let doc = merge_oauth(existing.as_deref(), new_oauth)?;
            let json = serde_json::to_string(&doc).map_err(AppError::Json)?;
            keychain::write_raw(&json)
        }
        #[cfg(not(target_os = "macos"))]
        CredsSource::Keychain => Err(AppError::Other(
            "Keychain credentials source is macOS-only".into(),
        )),
    }
}

/// Persist updated credentials to a file, preserving unknown top-level fields.
pub fn write_back(path: &Path, new_oauth: &OauthCreds) -> Result<()> {
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

    /// Like `write_creds`, but with no open handle on the file, so
    /// `write_back`'s atomic rename-over-destination succeeds on Windows.
    /// See [`crate::cache::closed_temp_file`].
    fn write_creds_closed(s: &str) -> (TempDir, std::path::PathBuf) {
        crate::cache::closed_temp_file("credentials.json", Some(s))
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
    // not a parse error. `read_from` is strict on every platform — explicit
    // paths never consult the Keychain (issues #14/#15) — so this needs no
    // macOS gate anymore.
    #[test]
    fn read_from_missing_file_is_io_error() {
        let path = std::path::Path::new("/nonexistent/ai-usagebar/.credentials.json");
        let err = read_from(path).unwrap_err();
        assert!(matches!(err, AppError::Io { .. }));
    }

    // --- issue #15: default-location Keychain fallback selection ------------
    // `read_default_with` takes the keychain reader as a closure, so these run
    // hermetically on any platform — no real Keychain, no real $HOME.

    const USABLE: &str = r#"{"claudeAiOauth":{
        "accessToken":"live-token","refreshToken":"rt","expiresAt": 9999999999999,
        "subscriptionType":"max","rateLimitTier":""}}"#;
    const UNUSABLE: &str = r#"{"claudeAiOauth":{
        "accessToken":"","refreshToken":"","expiresAt": 0,
        "subscriptionType":"","rateLimitTier":""}}"#;
    const KEYCHAIN_USABLE: &str = r#"{"claudeAiOauth":{
        "accessToken":"kc-token","refreshToken":"kc-rt","expiresAt": 9999999999999,
        "subscriptionType":"max","rateLimitTier":""}}"#;

    #[test]
    fn is_unusable_only_when_fully_dead() {
        let dead: CredentialsFile = serde_json::from_str(UNUSABLE).unwrap();
        assert!(is_unusable(&dead.claude_ai_oauth));
        // The v0.7.2 trusted-device shape (empty refreshToken, live
        // accessToken) must NOT count as unusable — file stays authoritative.
        let trusted: CredentialsFile = serde_json::from_str(
            r#"{"claudeAiOauth":{"accessToken":"live","refreshToken":"",
                "expiresAt": 9999999999999,"subscriptionType":"max","rateLimitTier":""}}"#,
        )
        .unwrap();
        assert!(!is_unusable(&trusted.claude_ai_oauth));
    }

    #[test]
    fn default_read_usable_file_wins_without_consulting_keychain() {
        let (_dir, path) = write_creds_closed(USABLE);
        let (creds, source) =
            read_default_with(&path, || panic!("keychain must not be consulted")).unwrap();
        assert_eq!(creds.claude_ai_oauth.access_token, "live-token");
        assert_eq!(source, CredsSource::File(path));
    }

    #[test]
    fn default_read_missing_file_falls_back_to_keychain() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("missing.json");
        let (creds, source) =
            read_default_with(&path, || Ok(Some(KEYCHAIN_USABLE.into()))).unwrap();
        assert_eq!(creds.claude_ai_oauth.access_token, "kc-token");
        assert_eq!(source, CredsSource::Keychain);
    }

    #[test]
    fn default_read_missing_file_without_keychain_is_io_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("missing.json");
        let err = read_default_with(&path, || Ok(None)).unwrap_err();
        assert!(matches!(err, AppError::Io { .. }));
    }

    #[test]
    fn a_locked_keychain_wins_over_the_file_missing_error() {
        // The regression this guards: `read_raw` mapped *every* `security`
        // failure to Ok(None), so a locked login Keychain looked identical to
        // "not logged in" and the user was told to run `claude` while their
        // credentials sat there intact.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("missing.json");
        let err = read_default_with(&path, || {
            Err(AppError::Credentials("the Keychain is locked".into()))
        })
        .unwrap_err();
        assert!(
            matches!(err, AppError::Credentials(ref m) if m.contains("locked")),
            "expected the Keychain error to surface, got {err:?}"
        );
    }

    #[test]
    fn default_read_unusable_file_prefers_usable_keychain() {
        // The #15 scenario: stale zeroed file shadowing fresh Keychain creds.
        let (_dir, path) = write_creds_closed(UNUSABLE);
        let (creds, source) =
            read_default_with(&path, || Ok(Some(KEYCHAIN_USABLE.into()))).unwrap();
        assert_eq!(creds.claude_ai_oauth.access_token, "kc-token");
        assert_eq!(source, CredsSource::Keychain);
    }

    #[test]
    fn default_read_unusable_file_kept_when_keychain_absent_or_dead() {
        let (_dir, path) = write_creds_closed(UNUSABLE);
        // No keychain item → the file result stands, source stays File.
        let (creds, source) = read_default_with(&path, || Ok(None)).unwrap();
        assert!(is_unusable(&creds.claude_ai_oauth));
        assert_eq!(source, CredsSource::File(path.clone()));
        // Keychain item just as dead → same.
        let (_, source) = read_default_with(&path, || Ok(Some(UNUSABLE.into()))).unwrap();
        assert_eq!(source, CredsSource::File(path));
    }

    #[test]
    fn default_read_unparsable_file_falls_back_to_keychain_else_errors() {
        let (_dir, path) = write_creds_closed("not json at all");
        let (creds, source) =
            read_default_with(&path, || Ok(Some(KEYCHAIN_USABLE.into()))).unwrap();
        assert_eq!(creds.claude_ai_oauth.access_token, "kc-token");
        assert_eq!(source, CredsSource::Keychain);
        // Without a keychain rescue, the original parse error surfaces.
        let err = read_default_with(&path, || Ok(None)).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
    }

    #[test]
    fn resolve_explicit_never_falls_back() {
        // An explicit target with a missing file is an I/O error even on a
        // machine whose Keychain holds valid creds — resolve() routes Explicit
        // through the strict read_from, which has no keychain path at all.
        let dir = TempDir::new().unwrap();
        let target = CredsTarget::Explicit(dir.path().join("missing.json"));
        assert!(matches!(resolve(&target).unwrap_err(), AppError::Io { .. }));
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
