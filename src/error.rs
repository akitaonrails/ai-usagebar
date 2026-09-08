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
    /// **The path is sanitized here rather than at each print site.** A path in
    /// this variant is not always a literal this program chose — it can carry a
    /// component from an account label, a vendor response, or an archive
    /// member — and `Display for Path` escapes nothing. Doing it at the
    /// `Display` covers every site that formats an `AppError`, including the
    /// ones written after this comment.
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

    /// A fetch failed after the vendor's plan label was already known (Claude
    /// OAuth `subscriptionType`, etc.). Display and [`Self::user_message`] are
    /// the inner error so UIs can still name the real cause; the plan is for
    /// surfaces that can show it without a quota snapshot.
    #[error("{source}")]
    WithPlan {
        plan: String,
        #[source]
        source: Box<AppError>,
    },
}

impl AppError {
    /// Convenience for non-pathful I/O.
    pub fn io_at(path: impl Into<PathBuf>, source: io::Error) -> Self {
        AppError::Io {
            path: path.into(),
            source,
        }
    }

    /// Attach a plan label already known from credentials. Empty labels are
    /// dropped so a card does not render a blank plan row.
    pub fn with_plan(self, plan: impl Into<String>) -> Self {
        let plan = plan.into();
        if plan.is_empty() {
            self
        } else {
            AppError::WithPlan {
                plan,
                source: Box::new(self),
            }
        }
    }

    /// Plan label carried by [`Self::WithPlan`], if any.
    pub fn plan(&self) -> Option<&str> {
        match self {
            AppError::WithPlan { plan, .. } => Some(plan.as_str()),
            _ => None,
        }
    }

    /// True for transient network errors that the widget should hide behind a
    /// "Loading…" rather than a "⚠".
    pub fn is_transient(&self) -> bool {
        match self {
            AppError::Transport(_) => true,
            AppError::WithPlan { source, .. } => source.is_transient(),
            _ => false,
        }
    }

    /// Render an error for a local UI or report without exposing an upstream
    /// authentication response body. Other errors retain their diagnostic text.
    pub fn user_message(&self) -> String {
        match self {
            AppError::WithPlan { source, .. } => source.user_message(),
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

    /// Asserted at the `Display`, not at a print site, because that is the
    /// whole point: a path in this variant can come from an account label, a
    /// vendor response, or an archive member, and there are a dozen places
    /// that format one.
    #[test]
    fn an_io_path_carrying_a_terminal_escape_renders_without_it() {
        let rendered = AppError::Io {
            path: PathBuf::from("/tmp/\x1b[2Kspoofed\nRESTORED: 0"),
            source: io::Error::other("disk full"),
        }
        .to_string();

        assert!(!rendered.contains('\u{1b}'), "{rendered:?}");
        assert!(
            !rendered.contains('\n'),
            "an embedded newline forges a line: {rendered:?}"
        );
        assert!(rendered.contains("disk full"), "{rendered}");
    }

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

    #[test]
    fn with_plan_keeps_the_inner_message_and_the_label() {
        let error = AppError::Http {
            status: 401,
            body: "invalid token".into(),
        }
        .with_plan("Claude Max 5x");
        assert_eq!(error.plan(), Some("Claude Max 5x"));
        let rendered = error.user_message();
        assert!(rendered.contains(AUTH_FAILURE_MESSAGE));
        assert!(!rendered.contains("invalid token"));
        assert!(!error.is_transient());
        assert!(
            AppError::Transport("timeout".into())
                .with_plan("Pro")
                .is_transient()
        );
    }
}
