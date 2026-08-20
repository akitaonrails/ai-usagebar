//! Shared error type. Vendors and renderers convert their failures into
//! `AppError` so the widget shell can decide whether to retry, fall back to
//! cache, show ⚠, or show "Loading…".

use std::io;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, AppError>;

pub const AUTH_FAILURE_MESSAGE: &str =
    "authentication rejected — credentials may be missing, expired, or invalid";

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Local I/O failed (cache write, credentials read, theme file, etc.).
    ///
    /// The path is rendered through the crate's sanitizer rather than through
    /// `Path`'s `Display`, which escapes nothing. Since Phase 5 a path here can
    /// be built component-by-component out of a manifest a hostile remote
    /// wrote, and this error reaches stderr from a dozen places — so the escape
    /// belongs in the one `Display` every one of them goes through, not in each
    /// print site that happened to remember (F-3).
    #[error(
        "io error at {}: {source}",
        crate::display::sanitize_untrusted_path(path)
    )]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// Generic I/O without a meaningful path (e.g. stdout writes).
    #[error(transparent)]
    IoBare(#[from] io::Error),

    /// A vendor's credentials file is missing, unreadable, or malformed.
    /// Distinct from `Io` because the widget treats it as "user must re-auth"
    /// rather than a transient failure.
    #[error("credentials error: {0}")]
    Credentials(String),

    /// HTTP request failed at the transport layer (DNS, TLS, timeout, connect).
    /// Maps to claudebar's "HTTP 000" — show `Loading…`, don't write
    /// `.last_error`, retry next tick.
    #[error("network transport error: {0}")]
    Transport(String),

    /// HTTP request reached the server but returned a non-2xx status.
    /// Carries the code + best-effort body so the widget can populate
    /// `.last_error` for the tooltip.
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },

    /// API returned 2xx but the body did not match our expected schema.
    /// Treated like an HTTP error for tooltip purposes, but logged separately
    /// because it signals undocumented-endpoint drift.
    #[error("schema mismatch: {0}")]
    Schema(String),

    /// JSON serialization/deserialization failure (config files, response bodies).
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// TOML config parse failure.
    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),

    /// Catch-all for unexpected conditions (cache lock contention, etc.).
    #[error("{0}")]
    Other(String),
}

impl AppError {
    /// Convenience for non-pathful I/O.
    pub fn io_at(path: impl Into<PathBuf>, source: io::Error) -> Self {
        AppError::Io {
            path: path.into(),
            source,
        }
    }

    /// True for transient network errors that the widget should hide behind a
    /// "Loading…" rather than a "⚠".
    pub fn is_transient(&self) -> bool {
        matches!(self, AppError::Transport(_))
    }

    /// Render an error for a local UI or report without exposing an upstream
    /// authentication response body. Other errors retain their diagnostic text.
    pub fn user_message(&self) -> String {
        match self {
            AppError::Http { status, .. } if matches!(status, 401 | 403) => {
                format!("HTTP {status}: {AUTH_FAILURE_MESSAGE}")
            }
            other => other.to_string(),
        }
    }
}

/// Map a reqwest error into the right variant. Connection-class failures
/// become `Transport` (transient); the rest become generic `Http`/`Other`.
impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() || err.is_connect() || err.is_request() {
            return AppError::Transport(err.to_string());
        }
        if let Some(status) = err.status() {
            return AppError::Http {
                status: status.as_u16(),
                body: err.to_string(),
            };
        }
        AppError::Other(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_does_not_expose_authentication_response_bodies() {
        for status in [401, 403] {
            let error = AppError::Http {
                status,
                body: "PANCEA user@example.test <credential>&token".into(),
            };
            let rendered = error.user_message();
            assert!(rendered.contains(AUTH_FAILURE_MESSAGE));
            assert!(!rendered.contains("PANCEA"));
            assert!(!rendered.contains("user@example.test"));
            assert!(!rendered.contains("&token"));
        }
    }

    #[test]
    fn user_message_preserves_non_authentication_diagnostics() {
        let error = AppError::Http {
            status: 500,
            body: "provider unavailable".into(),
        };
        assert!(error.user_message().contains("provider unavailable"));
    }
}
