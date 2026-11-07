//! What a vendor fetch produced: a snapshot, plus how much to trust it.
//!
//! Every vendor answers the same four questions — what the numbers are,
//! whether they came from a live call or a stale cache, what the last failure
//! was, and how old the payload is — so every vendor declared the same struct
//! and the same three helpers around it. Eighteen copies of a four-field
//! record is only tedious; eighteen copies of the *policy* is a hazard, and it
//! had already gone wrong twice in this family:
//!
//! - the cold-cache error, which five vendors replaced with a generic "no
//!   usable cache" while thirteen returned the real one;
//! - an unparseable cached payload, which some vendors reported as a cache
//!   parse error and others as the original fetch failure.
//!
//! Both are settled here once. A vendor supplies its snapshot type and a
//! closure that parses its own cache format; everything else is shared.

use std::future::Future;
use std::time::Duration;

use crate::cache::{Cache, MAX_STALE};
use crate::error::{AppError, Result};

/// A snapshot with its provenance.
#[derive(Debug, Clone)]
pub struct Outcome<T> {
    pub snapshot: T,
    /// The payload could not be refreshed — shown, but marked.
    pub stale: bool,
    /// The failure recorded by the most recent unsuccessful refresh, redacted
    /// by [`Cache::write_last_error`]. `None` means the last refresh worked.
    pub last_error: Option<(u16, String)>,
    /// How long ago the payload was written. `None` when unknown.
    pub cache_age: Option<Duration>,
}

impl<T> Outcome<T> {
    /// Straight off the wire: fresh, no recorded error, zero age.
    ///
    /// Note the deliberate asymmetry with [`Outcome::cached`] — a successful
    /// fetch clears `last_error` rather than reading it back, because the
    /// error it would read is the one this call just superseded.
    pub fn fresh(snapshot: T) -> Self {
        Self {
            snapshot,
            stale: false,
            last_error: None,
            cache_age: Some(Duration::ZERO),
        }
    }

    /// Parsed back out of the cache. The recorded error and the payload's age
    /// come from the cache, so a warm-cache render still shows why the last
    /// refresh failed.
    pub fn cached(snapshot: T, cache: &Cache, stale: bool) -> Self {
        Self {
            snapshot,
            // A 429 backoff can hand an expired payload through the normal
            // cache fast path. Keep the failure marker visible to frontends
            // even when the vendor passed `false` for that fast path.
            stale: stale || cache.is_stale() || cache.backoff_remaining().is_some(),
            last_error: cache.read_last_error(),
            cache_age: cache.payload_age(),
        }
    }

    /// Re-type the snapshot, keeping the provenance. This is how a vendor's
    /// own outcome becomes a [`crate::vendor::VendorOutcome`].
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Outcome<U> {
        Outcome {
            snapshot: f(self.snapshot),
            stale: self.stale,
            last_error: self.last_error,
            cache_age: self.cache_age,
        }
    }

    /// True only for an outcome built by [`Outcome::fresh`]: the payload came
    /// off the wire in this process, not out of the cache. The signal is
    /// `cache_age == Some(ZERO)` — a cached outcome's age is measured from
    /// its payload's mtime and is therefore nonzero. Consumers that must not
    /// act on replayed data (the notification check is the one today) gate
    /// on this rather than re-deriving freshness from `stale`, which is
    /// `false` for a within-TTL cache hit too.
    pub fn off_the_wire(&self) -> bool {
        !self.stale && self.cache_age == Some(Duration::ZERO)
    }
}

/// Serve the last good payload after a failed refresh, or give up with the
/// error that caused the failure.
///
/// `original` is returned — not a message about the cache — whenever there is
/// nothing to show. With no figure on screen the error *is* the output, so it
/// has to name the real cause: on a first run a rejected key, a `500` and an
/// empty cache are otherwise indistinguishable. A cached payload that will not
/// parse counts as nothing to show, and for the same reason reports `original`
/// rather than the parse failure, which is an internal detail the user cannot
/// act on.
///
/// `last_error` *overrides* what the cache recorded when it is `Some` — the
/// caller has just written a fresher diagnostic and holds the redacted copy.
/// `None` keeps whatever the cache holds, which is what a transient failure
/// wants: it records nothing, so the previous error should stay visible.
pub fn fallback<T>(
    cache: &Cache,
    last_error: Option<(u16, String)>,
    original: AppError,
    parse: impl FnOnce(&[u8]) -> Result<T>,
) -> Result<Outcome<T>> {
    let Some(bytes) = cache.fallback_payload(MAX_STALE)? else {
        return Err(original);
    };
    let Ok(snapshot) = parse(&bytes) else {
        return Err(original);
    };
    let mut outcome = Outcome::cached(snapshot, cache, true);
    if last_error.is_some() {
        outcome.last_error = last_error;
    }
    Ok(outcome)
}

/// The one fetch wrap for every key-based vendor, in the report/TUI path and
/// in the widget/CLI: the report's `api_key_and_cache` hands the key back
/// attached to the cache, the widget's `api_key_target` does the same, and
/// the vendors that resolve their key and build their own cache
/// (`AnthropicApi`, `Kimi`, `OpenCodeGo`, `Ollama`) pass it here — one
/// point, not one per vendor or surface. A provider's answer can quote the
/// key in the failure it reports (Z.AI returns the reason in `msg` on an
/// HTTP 200 with `success: false`), and when there is no figure on screen
/// the error is the output's whole text. The cache side is already redacted
/// at `write_last_error` (and the payload at `write_payload`); this strips
/// the key from the error that bubbles up instead. An empty secret is a
/// no-op for the variant whose credential value it does not hold (Kimi's
/// OAuth login).
pub(crate) async fn fetch_api_keyed<T, Fut>(fetch: Fut, key: &str) -> Result<T>
where
    Fut: Future<Output = Result<T>>,
{
    fetch
        .await
        .map_err(|error| redact_api_key_error(error, key))
}

/// Remove the key from the free text an error carries, preserving the
/// variant: a `Transport` error must stay transient, and `Http` keeps its
/// status. The key rides in the answer the provider sent — `Schema`, the
/// `Http` body, `Transport`, `Credentials`, `Other`, and the source behind
/// `WithPlan` — so only those are rewritten; local I/O and parse errors
/// carry paths and offsets, not a provider's answer, and pass through.
fn redact_api_key_error(error: AppError, key: &str) -> AppError {
    match error {
        AppError::Schema(message) => AppError::Schema(crate::display::redact_secret(&message, key)),
        AppError::Http { status, body } => AppError::Http {
            status,
            body: crate::display::redact_secret(&body, key),
        },
        AppError::Transport(message) => {
            AppError::Transport(crate::display::redact_secret(&message, key))
        }
        AppError::Credentials(message) => {
            AppError::Credentials(crate::display::redact_secret(&message, key))
        }
        AppError::Permission(message) => {
            AppError::Permission(crate::display::redact_secret(&message, key))
        }
        AppError::Other(message) => AppError::Other(crate::display::redact_secret(&message, key)),
        AppError::WithPlan { plan, source } => AppError::WithPlan {
            plan,
            source: Box::new(redact_api_key_error(*source, key)),
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("vendor"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    fn parse_ok(bytes: &[u8]) -> Result<String> {
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    fn parse_fails(_: &[u8]) -> Result<String> {
        Err(AppError::Schema("cached payload is not ours".into()))
    }

    /// The refactor that introduced this module replaced the hand-built
    /// records with `fresh`/`cached` by regex, and the regex missed six that
    /// used field shorthand — behaviourally identical, but they are how the
    /// policy drifts back apart. Constructing the record by hand is the thing
    /// to forbid, not any particular spelling of it.
    #[test]
    fn no_vendor_assembles_the_record_by_hand() {
        let mut sites = Vec::new();
        for file in crate::guard::rs_files_in("src") {
            if file.ends_with("outcome.rs") {
                continue;
            }
            let source = std::fs::read_to_string(&file).expect("readable module");
            for (n, line) in crate::guard::production_code(&source).lines().enumerate() {
                let line = line.trim();
                if line == "cache_age: Some(Duration::ZERO),"
                    || line == "cache_age: cache.payload_age(),"
                {
                    sites.push(format!("{}:{}", file.display(), n + 1));
                }
            }
        }
        assert!(
            sites.is_empty(),
            "build outcomes with `Outcome::fresh` or `Outcome::cached`, so the \
             provenance rules live in one place. Found: {sites:#?}"
        );
    }

    #[test]
    fn a_fresh_outcome_clears_the_recorded_error() {
        let out = Outcome::fresh("live");
        assert_eq!(out.snapshot, "live");
        assert!(!out.stale);
        assert_eq!(out.last_error, None);
        assert_eq!(out.cache_age, Some(Duration::ZERO));
    }

    #[test]
    fn a_cached_outcome_carries_the_recorded_error() {
        let (_td, cache) = fixture();
        cache.write_last_error(500, "upstream is down");

        let out = Outcome::cached("cached", &cache, true);

        assert!(out.stale);
        assert_eq!(out.last_error, Some((500, "upstream is down".to_string())));
    }

    #[test]
    fn a_backoff_cache_hit_is_marked_stale() {
        let (_td, cache) = fixture();
        cache.write_payload(b"last good").unwrap();
        cache.write_last_error(429, "rate limited");

        // Vendor fast paths call cached(..., false), including when the
        // shared cache serves old data during a 429 backoff.
        let out = Outcome::cached("last good", &cache, false);

        assert!(out.stale);
        assert_eq!(out.last_error.as_ref().map(|(code, _)| *code), Some(429));
    }

    #[test]
    fn a_cached_outcome_keeps_an_existing_stale_marker() {
        let (_td, cache) = fixture();
        cache.write_payload(b"last good").unwrap();
        cache.mark_stale();

        assert!(Outcome::cached("last good", &cache, false).stale);
    }

    #[test]
    fn map_re_types_the_snapshot_and_keeps_the_provenance() {
        let (_td, cache) = fixture();
        cache.write_last_error(429, "slow down");
        let out = Outcome::cached(7u8, &cache, true).map(|n| n as u32 * 2);

        assert_eq!(out.snapshot, 14u32);
        assert!(out.stale);
        assert_eq!(out.last_error, Some((429, "slow down".to_string())));
    }

    /// Freshness as the notification check needs it: only a wire-fresh
    /// outcome counts, and a within-TTL cache hit — which is also
    /// `stale: false` — must not.
    #[test]
    fn off_the_wire_is_true_only_for_wire_fresh_outcomes() {
        let fresh = Outcome::fresh("live");
        assert!(fresh.off_the_wire());

        let hand_cached = Outcome {
            snapshot: "cached",
            stale: false,
            last_error: None,
            cache_age: Some(Duration::from_secs(1)),
        };
        assert!(
            !hand_cached.off_the_wire(),
            "a within-TTL cache hit is not wire-fresh"
        );

        let stale = Outcome {
            stale: true,
            ..hand_cached
        };
        assert!(!stale.off_the_wire());

        let unknown_age = Outcome {
            snapshot: "cached",
            stale: false,
            last_error: None,
            cache_age: None,
        };
        assert!(!unknown_age.off_the_wire());
    }

    /// The regression this closes: five vendors returned
    /// `AppError::Other("… no usable cache")` here, so on a first run an
    /// expired key and an empty cache produced the same tooltip.
    #[test]
    fn no_cache_returns_the_error_that_caused_the_failure() {
        let (_td, cache) = fixture();

        let err = fallback(
            &cache,
            Some((401, "Authentication failed".into())),
            AppError::Http {
                status: 401,
                body: "Authentication failed".into(),
            },
            parse_ok,
        )
        .unwrap_err();

        assert!(
            matches!(err, AppError::Http { status: 401, .. }),
            "got {err:?}"
        );
    }

    /// A cached payload we cannot parse is no better than no payload — and the
    /// parse failure is not the user's problem, the failed fetch is. Vendors
    /// disagreed about this: some propagated the parse error with `?`.
    #[test]
    fn an_unparseable_cached_payload_also_reports_the_original_error() {
        let (_td, cache) = fixture();
        cache
            .write_payload(b"payload from another account")
            .unwrap();

        let err = fallback(
            &cache,
            None,
            AppError::Transport("network unreachable".into()),
            parse_fails,
        )
        .unwrap_err();

        assert!(
            matches!(&err, AppError::Transport(m) if m == "network unreachable"),
            "the cache parse failure masked the real cause: {err:?}"
        );
    }

    #[test]
    fn a_usable_cache_is_served_stale_with_the_fresher_diagnostic() {
        let (_td, cache) = fixture();
        cache.write_payload(b"last good").unwrap();
        cache.write_last_error(500, "an older failure");

        let out = fallback(
            &cache,
            Some((401, "Authentication failed".into())),
            AppError::Http {
                status: 401,
                body: "raw".into(),
            },
            parse_ok,
        )
        .unwrap();

        assert_eq!(out.snapshot, "last good");
        assert!(out.stale);
        assert_eq!(
            out.last_error,
            Some((401, "Authentication failed".to_string())),
            "the caller's fresher diagnostic must win over the recorded one"
        );
    }

    /// A transient failure records nothing, so it passes `None` and the error
    /// already on disk stays visible rather than being blanked.
    #[test]
    fn a_silent_fallback_keeps_the_error_already_recorded() {
        let (_td, cache) = fixture();
        cache.write_payload(b"last good").unwrap();
        cache.write_last_error(500, "an earlier failure");

        let out = fallback(
            &cache,
            None,
            AppError::Transport("network unreachable".into()),
            parse_ok,
        )
        .unwrap();

        assert_eq!(
            out.last_error,
            Some((500, "an earlier failure".to_string()))
        );
    }

    /// The report's and the widget's whole output when a fetch fails is the
    /// error, and a provider's answer can quote the key the fetch was sent
    /// with (Z.AI returns the reason in `msg` on an HTTP 200). The single
    /// wrap point both surfaces use strips the key before the error bubbles
    /// up…
    #[tokio::test]
    async fn fetch_api_keyed_redacts_the_key_from_the_error_a_fetch_returns() {
        let key = "test-key-value";
        let err = fetch_api_keyed(
            async {
                Err::<crate::vendor::VendorOutcome, _>(AppError::Schema(format!(
                    "zai: API reported failure (code 401, success false): bad key {key}"
                )))
            },
            key,
        )
        .await
        .unwrap_err();
        let rendered = err.user_message();
        assert!(!rendered.contains(key), "{rendered}");
        assert!(rendered.contains("[key]"), "{rendered}");
        // The variant survives the wrap: a schema mismatch stays one.
        assert!(matches!(err, AppError::Schema(_)), "{err:?}");
    }

    /// …and an error without the key passes through unchanged, with its
    /// variant (and its transience) intact.
    #[tokio::test]
    async fn fetch_api_keyed_leaves_an_error_without_the_key_alone() {
        let key = "test-key-value";
        let err = fetch_api_keyed(
            async {
                Err::<crate::vendor::VendorOutcome, _>(AppError::Transport("dns failed".into()))
            },
            key,
        )
        .await
        .unwrap_err();
        assert_eq!(err.user_message(), "network transport error: dns failed");
        assert!(err.is_transient(), "the variant must survive the wrap");
    }

    /// The redaction rewrites only the free text a provider sent, per
    /// variant: the `Http` status and the `WithPlan` label survive, the key
    /// does not, and an `Io` error with no provider text passes through.
    #[test]
    fn redact_api_key_error_covers_the_variants_that_carry_provider_text() {
        let key = "test-key-value";

        let http = redact_api_key_error(
            AppError::Http {
                status: 500,
                body: format!("boom {key}"),
            },
            key,
        );
        assert!(
            matches!(http, AppError::Http { status: 500, .. }),
            "{http:?}"
        );
        assert!(http.user_message().contains("[key]"), "{http:?}");
        assert!(!http.user_message().contains(key), "{http:?}");

        let wrapped = redact_api_key_error(
            AppError::Schema(format!("bad {key}")).with_plan("GLM Coding Pro"),
            key,
        );
        assert!(
            matches!(wrapped, AppError::WithPlan { ref plan, .. }
                if plan == "GLM Coding Pro"),
            "{wrapped:?}"
        );
        assert!(!wrapped.user_message().contains(key), "{wrapped:?}");

        let io = redact_api_key_error(
            AppError::Io {
                path: std::path::PathBuf::from("/tmp/ai-usagebar"),
                source: std::io::Error::other("disk full"),
            },
            key,
        );
        assert!(matches!(io, AppError::Io { .. }), "{io:?}");
        assert!(io.user_message().contains("disk full"), "{io:?}");
    }
}
