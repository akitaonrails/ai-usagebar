//! The GitHub failure taxonomy — and, for each failure, what to do about it.
//!
//! The *type* is frozen by plan 3-01 and every later plan matches on it, so the
//! variant set below is closed for Phase 3: plan 3-03 fills in the classifier's
//! arithmetic and the message text, and adds no variant. [`Conflict`] is unreached
//! in this phase and exists so Phase 4's compare-and-swap pointer write does not
//! have to widen an enum three other files match on.
//!
//! D-06 is the whole point of this file: a failure that does not name its fix
//! costs the user a support round trip. The two failures that most need telling
//! apart are 401 and 403 — one means the stored token is dead and should be
//! cleared, the other means it is alive but under-permissioned or rate-limited,
//! and clearing it would destroy a working credential.
//!
//! Two rules hold everywhere below:
//!
//! - **The token never appears.** Not a value, not a prefix, not through a
//!   `Debug`. Presence and source are reportable; the secret is not (T-3-15).
//! - **The response body is untrusted input.** It is sanitized through
//!   [`sanitize_untrusted_field`], truncated, and interpolated as a *value* —
//!   never used as a format string (T-3-17).
//!
//! [`Conflict`]: GithubError::Conflict

use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use reqwest::header::HeaderMap;

use crate::display::sanitize_untrusted_field;
use crate::error::AppError;

/// Never retry sooner than this. GitHub's secondary limits are per-minute, and
/// a hot loop against a rate limiter is how a token earns a longer one.
pub const MIN_RETRY_DELAY: Duration = Duration::from_secs(60);

/// Never wait longer than this, whatever a header claims. The primary rate
/// limit is a one-hour window, so an hour is the longest wait that can still be
/// the truth; a `retry-after` of a million seconds is a garbled or hostile
/// header, and honouring it would park the process for eleven days (T-3-14).
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(3600);

/// The exact permission pair a fine-grained PAT needs, worded identically in
/// every message that names it (and in `token::resolve`'s exhausted-chain text).
const PERMISSIONS: &str = "Contents: read/write and Metadata: read";

/// Where a replacement token is issued.
const NEW_TOKEN_URL: &str = "https://github.com/settings/personal-access-tokens";

/// Everything a GitHub request can fail as. **Seven variants, frozen.**
#[derive(Debug, thiserror::Error)]
pub enum GithubError {
    /// 401 — the token is missing, revoked, or expired.
    #[error("GitHub rejected the token (401): {message}")]
    Unauthorized { message: String },

    /// 403 or 429 carrying rate-limit headers. `retry_after` is how long to
    /// wait, computed by [`retry_delay`] from `retry-after` / `x-ratelimit-reset`.
    #[error("GitHub rate-limited this token: {message}")]
    RateLimited {
        retry_after: Duration,
        message: String,
    },

    /// 403 *without* rate-limit headers — a permission the token does not carry.
    #[error("GitHub refused this request (403): {message}")]
    Forbidden { message: String },

    /// 404 — the repository is absent, or the token is not scoped to it.
    /// GitHub deliberately returns the same status for both.
    #[error("{message}")]
    NotFound { message: String },

    /// 409 — a compare-and-swap precondition failed. Unreached in Phase 3.
    #[error("GitHub reported a conflicting remote state (409): {message}")]
    Conflict { message: String },

    /// Any other non-2xx.
    #[error("GitHub returned an unexpected HTTP {status}: {message}")]
    Unexpected { status: u16, message: String },

    /// DNS, TLS, connect, or timeout — the request never got a status.
    #[error("could not reach GitHub: {message}")]
    Transport { message: String },
}

/// Every variant is a failure. None of them may land on a success-shaped value.
impl From<GithubError> for AppError {
    fn from(err: GithubError) -> Self {
        // [`actionable`], not `to_string()`. This is the **only** conversion
        // between a `GithubError` and the error a user reads, so it is the one
        // place D-06's "every failure path names the fix" can be enforced
        // rather than remembered — and until plan 3-07 wired it, `actionable`
        // had no call site at all and the whole message table was unreachable.
        let body = actionable(&err);
        match err {
            GithubError::Unauthorized { .. } => AppError::Credentials(body),
            GithubError::RateLimited { .. } => AppError::Http { status: 429, body },
            GithubError::Forbidden { .. } => AppError::Http { status: 403, body },
            GithubError::NotFound { .. } => AppError::Http { status: 404, body },
            GithubError::Conflict { .. } => AppError::Http { status: 409, body },
            GithubError::Unexpected { status, .. } => AppError::Http { status, body },
            GithubError::Transport { .. } => AppError::Transport(body),
        }
    }
}

/// Turn a non-2xx response into the right variant.
///
/// `now` is a parameter and never `Utc::now()` inside — the rate-limit reset
/// header is an absolute instant, and a test that cannot pin the clock cannot
/// assert the delay. Same shape as `antigravity::parse_cache_at`.
///
/// The 403/429 split is the discrimination this whole file exists for. A 403
/// carrying a limit signal is a *wait*; a 403 without one is a *missing
/// permission*. Reporting the first as "your token lacks permission" sends the
/// user to re-issue a working token; reporting the second as "wait and retry"
/// loops forever. A 429 is always a wait — the status means nothing else.
///
/// 422 is deliberately *not* special-cased: a `PUT` that omits `sha` against an
/// existing file answers 422 rather than 409, but 422 is also GitHub's generic
/// validation failure, so mapping it onto [`Conflict`](GithubError::Conflict)
/// here would mislabel every malformed request. It lands in
/// [`Unexpected`](GithubError::Unexpected) carrying its status, and Phase 4
/// routes it onto the conflict path at the one call site that knows a `sha` was
/// omitted.
pub fn classify(
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
    now: DateTime<Utc>,
) -> GithubError {
    let message = message_of(body);
    match status.as_u16() {
        401 => GithubError::Unauthorized { message },
        429 => GithubError::RateLimited {
            retry_after: retry_delay(headers, 0, now),
            message,
        },
        403 if has_limit_signal(headers) => GithubError::RateLimited {
            retry_after: retry_delay(headers, 0, now),
            message,
        },
        403 => GithubError::Forbidden { message },
        404 => GithubError::NotFound { message },
        409 => GithubError::Conflict { message },
        status => GithubError::Unexpected { status, message },
    }
}

/// How long to wait before attempt `attempt + 1`, in GitHub's own terms where
/// it gives them (research §4.3):
///
/// 1. `retry-after`, in seconds;
/// 2. else, when `x-ratelimit-remaining` is `0`, the interval from `now` to the
///    `x-ratelimit-reset` UTC epoch second;
/// 3. else exponential backoff over `attempt`, with jitter so N clients that
///    tripped the same limit do not return in lockstep.
///
/// Clamped to `[MIN_RETRY_DELAY, MAX_RETRY_DELAY]` at the end, whichever rung
/// produced it: a reset already in the past yields a floor rather than a zero,
/// and a hostile header yields the ceiling rather than a week (T-3-14).
pub fn retry_delay(headers: &HeaderMap, attempt: u32, now: DateTime<Utc>) -> Duration {
    let secs = header_num::<u64>(headers, "retry-after")
        .or_else(|| {
            (header_num::<u64>(headers, "x-ratelimit-remaining") == Some(0))
                .then(|| header_num::<i64>(headers, "x-ratelimit-reset"))
                .flatten()
                .map(|reset| reset.saturating_sub(now.timestamp()).max(0) as u64)
        })
        .unwrap_or_else(|| {
            // 60, 120, 240 … capped at the 64× step, plus up to 25% jitter.
            let base = MIN_RETRY_DELAY.as_secs() << attempt.min(6);
            base.saturating_add(jitter_upto(base / 4))
        });
    Duration::from_secs(secs).clamp(MIN_RETRY_DELAY, MAX_RETRY_DELAY)
}

/// Does this response say "you are being limited" rather than "you may not"?
///
/// A `retry-after` that is *present but garbled* still counts: a hostile or
/// broken remote must not be able to flip a rate limit into "your token lacks
/// permission", which would send the user off to re-issue a working token. The
/// garbled value simply fails to parse in [`retry_delay`] and falls through to
/// backoff.
fn has_limit_signal(headers: &HeaderMap) -> bool {
    headers.contains_key("retry-after")
        || header_num::<u64>(headers, "x-ratelimit-remaining") == Some(0)
}

fn header_num<T: std::str::FromStr>(headers: &HeaderMap, name: &str) -> Option<T> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// Uniform in `0..max`, from the OS CSPRNG already in the tree. No `rand`.
fn jitter_upto(max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    let mut bytes = [0u8; 8];
    match getrandom::fill(&mut bytes) {
        // ponytail: modulo bias over a 60-ish range of a u64 is irrelevant for
        // jitter; it would matter for a nonce, and `crypto::fill` is that path.
        Ok(()) => u64::from_le_bytes(bytes) % max,
        Err(_) => max / 2,
    }
}

/// The one place a `reqwest` failure becomes a [`GithubError`].
/// `Client::get_json` is its only call site.
///
/// The three cases reqwest can actually tell apart get their own words, because
/// they have different fixes: a timeout is a slow link or a stalled endpoint, a
/// connect failure is DNS/TLS/a blocked port (reqwest reports a failed TLS
/// handshake as a connect error — there is no separate predicate to branch on),
/// and a request that could not even be built never left the process.
pub fn from_transport(e: &reqwest::Error) -> GithubError {
    let what = if e.is_timeout() {
        "GitHub did not answer before the request timed out"
    } else if e.is_connect() {
        "no connection could be opened — DNS, TLS, or a blocked port"
    } else if e.is_request() {
        "the request was never sent"
    } else {
        "the request did not complete"
    };
    GithubError::Transport {
        message: format!("{what} ({})", excerpt(&e.to_string())),
    }
}

/// The user-facing line for a failure: what happened *and* what to do about it.
///
/// Every arm ends in something the user can do, and no arm interpolates the
/// token, a header value, or an unbounded body (T-3-15). The `message` each one
/// carries is already sanitized and truncated by [`message_of`].
pub fn actionable(err: &GithubError) -> String {
    match err {
        // The one status where the token that was *sent* is provably useless.
        //
        // It says nothing about clearing. This function knows the status and not
        // the `TokenSource`, and "the stored token will be cleared" was false in
        // both directions: it fired for a token this tool never stored, and it
        // promised a deletion the env and `gh` paths must not perform (F-1).
        // `token::clear_note`, at the one call site that knows the source, says
        // what was actually done.
        GithubError::Unauthorized { message } => format!(
            "GitHub rejected the sync token (401): {message}. That token is dead. Issue a \
             replacement at {NEW_TOKEN_URL}/new — a fine-grained PAT scoped to the single sync \
             repository, with {PERMISSIONS} — then supply it in AI_USAGEBAR_SYNC_TOKEN or \
             ~/.config/ai-usagebar/sync-token and re-run `ai-usagebar sync setup`."
        ),
        // Deliberately says nothing about clearing or re-issuing: this token
        // works, it is just missing a grant (T-3-16).
        GithubError::Forbidden { message } => format!(
            "GitHub refused this request (403): {message}. The token is valid — keep it — but it \
             lacks a permission on this repository. Edit it at {NEW_TOKEN_URL}, confirm the \
             repository is in its selected list, and grant {PERMISSIONS}. Then re-run the same \
             command."
        ),
        GithubError::RateLimited {
            retry_after,
            message,
        } => format!(
            "GitHub rate-limited this token: {message}. Nothing is wrong with the token or the \
             repository. Wait {} and re-run the same command; the limit clears on its own.",
            humanize(*retry_after)
        ),
        // Both causes, because GitHub will not say which — it answers 404 rather
        // than 403 for a private repository a token cannot see. And no offer to
        // create anything: a 404 can be a name squat (REPO-03).
        GithubError::NotFound { message } => format!(
            "GitHub returned 404 for this repository: {message}. That is two different problems \
             wearing one status, and GitHub will not say which: either the repository does not \
             exist under that name, or the token is not scoped to it — GitHub answers 404 for a \
             private repository a token cannot see. Check `repo` under [sync] in config.toml, \
             then check the token's repository access at {NEW_TOKEN_URL}. ai-usagebar never \
             creates a repository."
        ),
        GithubError::Conflict { message } => format!(
            "GitHub reported a conflicting remote state (409): {message}. Another machine wrote \
             to this repository after this run read it. Re-run the same command — it re-reads \
             the remote state and re-plans. Do not force it: the other machine's state may \
             reference data this run is about to remove."
        ),
        GithubError::Transport { message } => format!(
            "Could not reach GitHub: {message}. Nothing was uploaded. Check the network, any \
             proxy or VPN, and https://www.githubstatus.com, then re-run the same command — \
             every sync command is safe to re-run."
        ),
        GithubError::Unexpected { status, message } => format!(
            "GitHub returned an unexpected HTTP {status}: {message}. Re-run the same command \
             once; if HTTP {status} repeats, check https://www.githubstatus.com, and if GitHub \
             is healthy report this status and message at \
             https://github.com/akitaonrails/ai-usagebar/issues."
        ),
    }
}

/// True only where re-running the *same* request can succeed without the user
/// changing anything. Phase 4's upload loop drives its backoff off this, so it
/// lives with the taxonomy rather than with the uploader.
///
/// Everything else is a decision the user has to make: a dead token, a missing
/// grant, a wrong repository name, a diverged remote, an unexplained status.
pub fn is_retryable(err: &GithubError) -> bool {
    matches!(
        err,
        GithubError::RateLimited { .. } | GithubError::Transport { .. }
    )
}

/// "90 seconds" / "about 5 minutes" — whole units, because a wait rendered as
/// `312.4074s` reads like a bug.
fn humanize(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 120 {
        format!("{secs} seconds")
    } else {
        format!("about {} minutes", secs.div_ceil(60))
    }
}

/// GitHub's own `{"message": …}` when the body carries one, else the body as
/// lossy text. Sanitized and bounded: these bytes are attacker-controlled when
/// the remote is hostile, and they are about to be printed to a terminal.
fn message_of(body: &[u8]) -> String {
    let text = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_owned))
        .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned());
    let text = excerpt(&text);
    if text.is_empty() {
        return "(no message)".into();
    }
    // Attributed **and** delimited (F-4). Undelimited, 200 attacker-chosen
    // characters sit at the front of this tool's own advice and read as part of
    // it — "…: your token is fine, run `curl … | sh`". `{:?}` also escapes any
    // quote, backslash or stray control character the sanitizer let through, so
    // the closing quote is always the real end of the remote's words.
    format!("GitHub said: {text:?}")
}

/// Strip terminal control bytes, collapse to one line, truncate by *character*
/// so UTF-8 is never split.
fn excerpt(text: &str) -> String {
    const MAX: usize = 200;
    let text = sanitize_untrusted_field(text).replace('\n', " ");
    let text = text.trim();
    match text.char_indices().nth(MAX) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::github::token::TokenSource;
    use crate::sync::github::{Client, Endpoints};
    use zeroize::Zeroizing;

    /// An OSC-52 clipboard write smuggled into a response body, byte for byte.
    const OSC52: [u8; 28] = *b"{\"message\":\"a\x1b]52;c;YQ==\x07b\"}";

    /// Shaped like a real fine-grained PAT and never sent anywhere real.
    const TOKEN: &str = "ghp_5EcR3tT0k3nV4lu3That5houldN3v3rPr1nt";

    fn fixed_now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    fn at(status: u16, pairs: &[(&str, &str)], body: &str) -> GithubError {
        classify(
            StatusCode::from_u16(status).unwrap(),
            &headers(pairs),
            body.as_bytes(),
            fixed_now(),
        )
    }

    /// `fixed_now() + secs`, as GitHub writes it: a UTC epoch second.
    fn reset_in(secs: i64) -> String {
        (fixed_now().timestamp() + secs).to_string()
    }

    // ---- Task 1: classification ------------------------------------------

    #[test]
    fn each_status_lands_on_its_own_variant() {
        assert!(matches!(
            at(401, &[], r#"{"message":"Bad credentials"}"#),
            GithubError::Unauthorized { message } if message == r#"GitHub said: "Bad credentials""#
        ));
        assert!(matches!(at(404, &[], "{}"), GithubError::NotFound { .. }));
        assert!(matches!(at(409, &[], "{}"), GithubError::Conflict { .. }));
        assert!(matches!(
            at(500, &[], "boom"),
            GithubError::Unexpected { status: 500, .. }
        ));
        // 422 is Phase 4's problem, and it keeps its own status meanwhile.
        assert!(matches!(
            at(422, &[], "{}"),
            GithubError::Unexpected { status: 422, .. }
        ));
    }

    /// The discrimination the file exists for: a limit signal means wait, its
    /// absence means the token is missing a grant (T-3-16).
    #[test]
    fn a_403_is_a_wait_only_when_the_response_says_so() {
        assert!(matches!(
            at(403, &[("retry-after", "90")], "{}"),
            GithubError::RateLimited { retry_after, .. } if retry_after == Duration::from_secs(90)
        ));
        assert!(matches!(
            at(
                403,
                &[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", &reset_in(300)),
                ],
                "{}"
            ),
            GithubError::RateLimited { retry_after, .. } if retry_after == Duration::from_secs(300)
        ));
        // Headers that exist but do not say "limited" are not a limit signal.
        assert!(matches!(
            at(403, &[("x-ratelimit-remaining", "4999")], "{}"),
            GithubError::Forbidden { .. }
        ));
        assert!(matches!(at(403, &[], "{}"), GithubError::Forbidden { .. }));
    }

    /// A 429 means one thing only. It never reads as a missing permission, with
    /// or without headers, and it takes its delay from the same three rungs.
    #[test]
    fn a_429_is_always_a_wait_and_uses_the_same_delay_source() {
        assert!(matches!(
            at(429, &[("retry-after", "90")], "{}"),
            GithubError::RateLimited { retry_after, .. } if retry_after == Duration::from_secs(90)
        ));
        assert!(matches!(
            at(429, &[], "{}"),
            GithubError::RateLimited { retry_after, .. } if retry_after >= MIN_RETRY_DELAY
        ));
    }

    #[test]
    fn the_delay_ladder_runs_in_order_and_is_clamped_at_both_ends() {
        let delay =
            |pairs: &[(&str, &str)], attempt| retry_delay(&headers(pairs), attempt, fixed_now());

        // Rung 1 wins over rung 2 even when both are present.
        assert_eq!(
            delay(
                &[
                    ("retry-after", "120"),
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", &reset_in(900)),
                ],
                0
            ),
            Duration::from_secs(120)
        );
        // Rung 2 only applies when remaining is actually zero.
        assert_eq!(
            delay(
                &[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", &reset_in(1800)),
                ],
                0
            ),
            Duration::from_secs(1800)
        );
        // A reset in the past is a floor, never a zero or a negative.
        assert_eq!(
            delay(
                &[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", &reset_in(-5_000)),
                ],
                0
            ),
            MIN_RETRY_DELAY
        );
        // T-3-14: a hostile header cannot park the process for eleven days.
        assert_eq!(delay(&[("retry-after", "999999999")], 0), MAX_RETRY_DELAY);
        assert_eq!(
            delay(
                &[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", "99999999999"),
                ],
                0
            ),
            MAX_RETRY_DELAY
        );
        // Garbage on any rung falls through to backoff rather than to a panic.
        for junk in [
            vec![("retry-after", "tomorrow")],
            vec![("retry-after", "-1")],
            vec![
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset", "soon"),
            ],
            vec![("x-ratelimit-remaining", "nan")],
        ] {
            let d = delay(&junk, 0);
            assert!((MIN_RETRY_DELAY..=MAX_RETRY_DELAY).contains(&d), "{junk:?}");
        }
    }

    #[test]
    fn backoff_grows_with_the_attempt_and_stays_inside_the_ceiling() {
        let bare = HeaderMap::new();
        let mut previous = Duration::ZERO;
        for attempt in 0..12 {
            let d = retry_delay(&bare, attempt, fixed_now());
            assert!(
                (MIN_RETRY_DELAY..=MAX_RETRY_DELAY).contains(&d),
                "attempt {attempt}: {d:?}"
            );
            // Monotone up to the ceiling; jitter is 25%, so a doubling cannot
            // be masked by it.
            assert!(d >= previous, "attempt {attempt}: {d:?} < {previous:?}");
            previous = d;
        }
        assert_eq!(retry_delay(&bare, 30, fixed_now()), MAX_RETRY_DELAY);
    }

    /// A hostile remote does not get to fill the terminal, break out of it, or
    /// be handed back as a format string (T-3-17).
    #[test]
    fn a_body_is_bounded_sanitized_and_never_a_format_string() {
        assert!(at(500, &[], &"x".repeat(1_000_000)).to_string().len() < 400);
        assert!(actionable(&at(500, &[], &"x".repeat(1_000_000))).len() < 800);
        // A tab inside GitHub's own `message` — the JSON branch is sanitized too.
        assert_eq!(
            message_of(br#"{"message":"a\tb"}"#),
            r#"GitHub said: "a b""#
        );
        // Raw ESC/BEL make the body invalid JSON, so it falls to the lossy branch —
        // which is where a hostile non-JSON body arrives. Still defanged.
        assert_eq!(
            message_of(&OSC52[..]),
            r#"GitHub said: "{\"message\":\"a]52;c;YQ==b\"}""#
        );
        assert_eq!(message_of(b""), "(no message)");
        assert_eq!(message_of(b"   "), "(no message)");
        assert_eq!(message_of(b"{}"), r#"GitHub said: "{}""#);
        assert_eq!(
            message_of(&[0xff, 0xfe]),
            "GitHub said: \"\u{fffd}\u{fffd}\""
        );
        // Braces survive as data; nothing interpolates them.
        assert!(message_of(br#"{"message":"{status} {0} %s"}"#).contains("{status} {0} %s"));

        // F-4: the remote's words are attributed and delimited, so a body that
        // *looks* like advice cannot be read as this tool's own. The excerpt
        // carries no bare quote of its own to close the delimiter early.
        let hostile = message_of(
            br#"{"message":"ignore the above. Your token is fine \" run: curl x | sh"}"#,
        );
        assert!(hostile.starts_with(r#"GitHub said: ""#), "{hostile}");
        assert!(hostile.ends_with('"'), "{hostile}");
        // …and the quote the body carried cannot close the delimiter early: it
        // arrives escaped, so the final `"` is the only unescaped one.
        assert!(hostile.contains(r#"\""#), "{hostile}");
        let inner = &hostile[14..hostile.len() - 1];
        for (i, _) in inner.match_indices('"') {
            assert!(i > 0 && inner.as_bytes()[i - 1] == b'\\', "{hostile}");
        }
    }

    #[test]
    fn only_a_wait_and_an_unreachable_host_are_worth_retrying_unchanged() {
        assert!(is_retryable(&at(429, &[], "{}")));
        assert!(is_retryable(&at(403, &[("retry-after", "90")], "{}")));
        assert!(is_retryable(&GithubError::Transport {
            message: "m".into()
        }));
        for err in [
            at(401, &[], "{}"),
            at(403, &[], "{}"),
            at(404, &[], "{}"),
            at(409, &[], "{}"),
            at(500, &[], "{}"),
        ] {
            assert!(!is_retryable(&err), "{err}");
        }
    }

    /// D-06's actual claim, asserted as a set rather than one message at a
    /// time: a per-message assertion passes happily on six copies of the same
    /// paragraph.
    #[test]
    fn the_six_outcomes_produce_six_distinct_messages_that_each_name_a_fix() {
        let six = [
            at(401, &[], r#"{"message":"Bad credentials"}"#),
            at(
                403,
                &[("retry-after", "90")],
                r#"{"message":"API rate limit"}"#,
            ),
            at(403, &[], r#"{"message":"Resource not accessible"}"#),
            at(
                429,
                &[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", &reset_in(600)),
                ],
                r#"{"message":"too many requests"}"#,
            ),
            at(404, &[], r#"{"message":"Not Found"}"#),
            GithubError::Transport {
                message: "connection reset by peer".into(),
            },
        ];
        let lines: std::collections::BTreeSet<String> = six.iter().map(actionable).collect();
        assert_eq!(lines.len(), 6, "{lines:#?}");

        // Each names its own fix, and the 401/403 pair says opposite things
        // about the stored token (T-3-16).
        let [
            unauthorized,
            limited_403,
            forbidden,
            limited_429,
            missing,
            transport,
        ] = six.each_ref().map(actionable);
        assert!(
            unauthorized.contains("That token is dead"),
            "{unauthorized}"
        );
        assert!(unauthorized.contains(PERMISSIONS), "{unauthorized}");
        // F-1: this arm knows the status, not the store the value came from, so
        // it promises nothing about clearing. `setup::clear_if_dead` appends the
        // sentence that does — after acting.
        assert!(!unauthorized.contains("cleared"), "{unauthorized}");
        assert!(!forbidden.contains("cleared"), "{forbidden}");
        assert!(forbidden.contains("keep it"), "{forbidden}");
        assert!(forbidden.contains(PERMISSIONS), "{forbidden}");
        assert!(limited_403.contains("Wait 90 seconds"), "{limited_403}");
        assert!(limited_429.contains("about 10 minutes"), "{limited_429}");
        assert!(missing.contains("does not exist"), "{missing}");
        assert!(missing.contains("not scoped to it"), "{missing}");
        assert!(!missing.contains("gh repo create"), "{missing}");
        assert!(transport.contains("re-run"), "{transport}");
        for line in [
            &unauthorized,
            &limited_403,
            &forbidden,
            &limited_429,
            &missing,
            &transport,
        ] {
            assert!(line.contains("re-run") || line.contains("Check"), "{line}");
        }
    }

    // ---- The same classification, over a real socket ----------------------

    fn client_at(base: &str) -> Client {
        Client::new(
            &Endpoints {
                api_base: base.into(),
                uploads_base: base.into(),
            },
            Zeroizing::new(TOKEN.into()),
            TokenSource::Env,
        )
        .unwrap()
    }

    /// One real request through `Client::get_json`, classified exactly as
    /// production classifies it.
    async fn over_the_wire(status: usize, pairs: &[(&str, &str)], body: &str) -> GithubError {
        let mut server = mockito::Server::new_async().await;
        let mut mock = server
            .mock("GET", "/repos/o/n")
            .with_status(status)
            .with_body(body);
        for (k, v) in pairs {
            mock = mock.with_header(*k, v);
        }
        let mock = mock.create_async().await;

        let (status, headers, body) = client_at(&server.url())
            .get_json("/repos/o/n")
            .await
            .expect("a non-2xx is still a response");
        mock.assert_async().await;
        classify(status, &headers, &body, fixed_now())
    }

    #[tokio::test]
    async fn the_wire_produces_the_same_six_classifications() {
        assert!(matches!(
            over_the_wire(401, &[], r#"{"message":"Bad credentials"}"#).await,
            GithubError::Unauthorized { .. }
        ));
        assert!(matches!(
            over_the_wire(403, &[("retry-after", "120")], "{}").await,
            GithubError::RateLimited { retry_after, .. } if retry_after == Duration::from_secs(120)
        ));
        assert!(matches!(
            over_the_wire(403, &[], r#"{"message":"Resource not accessible"}"#).await,
            GithubError::Forbidden { .. }
        ));
        assert!(matches!(
            over_the_wire(
                429,
                &[
                    ("x-ratelimit-remaining", "0"),
                    ("x-ratelimit-reset", &reset_in(300)),
                ],
                "{}",
            )
            .await,
            GithubError::RateLimited { retry_after, .. } if retry_after == Duration::from_secs(300)
        ));
        assert!(matches!(
            over_the_wire(404, &[], r#"{"message":"Not Found"}"#).await,
            GithubError::NotFound { .. }
        ));
        assert!(matches!(
            over_the_wire(500, &[], "upstream exploded").await,
            GithubError::Unexpected { status: 500, .. }
        ));
    }

    /// The sixth outcome has no status at all. A closed loopback port is the
    /// cheapest hermetic version of "the request never reached GitHub".
    #[tokio::test]
    async fn a_dead_port_becomes_a_transport_failure_with_retry_guidance() {
        let e = reqwest::Client::new()
            .get("http://127.0.0.1:1/repos/o/n")
            .send()
            .await
            .expect_err("nothing listens there");
        let err = from_transport(&e);
        assert!(matches!(err, GithubError::Transport { .. }), "{err}");
        assert!(is_retryable(&err));
        let line = actionable(&err);
        assert!(line.contains("Could not reach GitHub"), "{line}");
        assert!(line.contains("Nothing was uploaded"), "{line}");
        assert!(line.contains("re-run"), "{line}");
        // The `Client` path must reach the same place, as a transient AppError.
        let app = client_at("http://127.0.0.1:1")
            .get_json("/repos/o/n")
            .await
            .expect_err("nothing listens there");
        assert!(app.is_transient(), "{app}");
    }

    // ---- Task 2: the two guards ------------------------------------------

    fn one_of_each() -> [GithubError; 7] {
        [
            GithubError::Unauthorized {
                message: "Bad credentials".into(),
            },
            GithubError::RateLimited {
                retry_after: Duration::from_secs(90),
                message: "API rate limit exceeded".into(),
            },
            GithubError::Forbidden {
                message: "Resource not accessible by personal access token".into(),
            },
            GithubError::NotFound {
                message: "Not Found".into(),
            },
            GithubError::Conflict {
                message: "is at 0000 but expected 1111".into(),
            },
            GithubError::Unexpected {
                status: 500,
                message: "Server Error".into(),
            },
            GithubError::Transport {
                message: "connection reset by peer".into(),
            },
        ]
    }

    /// T-3-18: a failure reported as success is the worst outcome in this file.
    /// The widget's exit-0 invariant belongs to the *widget*; `sync` must exit
    /// non-zero, and every one of these has to land somewhere the CLI renders
    /// as a failure.
    #[test]
    fn every_variant_converts_into_a_failure_app_error() {
        for err in one_of_each() {
            let text = actionable(&err);
            assert!(!text.is_empty());
            let app: AppError = err.into();
            assert!(
                matches!(
                    app,
                    AppError::Credentials(_) | AppError::Http { .. } | AppError::Transport(_)
                ),
                "{app}"
            );
            // No failure arm may render as an empty or success-shaped line.
            assert!(!app.to_string().is_empty());
            assert!(!app.user_message().is_empty());
        }
    }

    /// T-3-15. A prefix is still a secret: "we only log the first few
    /// characters" is the exact shortcut this asserts against.
    #[test]
    fn no_rendering_of_any_failure_contains_the_token_or_a_prefix_of_it() {
        let client = client_at("https://example.invalid");
        let prefix = &TOKEN[..8];
        let mut rendered = vec![format!("{client:?}")];
        for err in one_of_each() {
            rendered.push(actionable(&err));
            rendered.push(err.to_string());
            rendered.push(format!("{err:?}"));
            let app: AppError = err.into();
            rendered.push(app.to_string());
            rendered.push(app.user_message());
        }
        for line in &rendered {
            assert!(!line.contains(TOKEN), "leaked the token: {line}");
            assert!(!line.contains(prefix), "leaked a token prefix: {line}");
        }
    }
}
