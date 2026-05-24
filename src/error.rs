//! Shared error type. Vendors and renderers convert their failures into
//! `AppError` so the widget shell can decide whether to retry, fall back to
//! cache, show ⚠, or show "Loading…".

use std::io;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Local I/O failed (cache write, credentials read, theme file, etc.).
    #[error("io error at {path}: {source}")]
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
