//! The private-repo gate: the one thing that must be true before a byte moves.
//!
//! D-04 says the check runs *immediately before every push*, never from a cached
//! fact — a repository can be flipped public from the web UI at any moment. Two
//! things here make that structural rather than a convention:
//!
//! - [`assert_pushable`] is the **sole** constructor of [`PushClearance`], whose
//!   field is private and which is neither `Clone` nor `Copy`. A clearance
//!   cannot be forged, and cannot be duplicated into a cache.
//! - [`PushClearance::spend`] turns "immediately" into arithmetic that cannot be
//!   skipped. It **consumes** the clearance and is the only way to obtain a
//!   [`Pushing`], which every write verb takes. There is no `&self` freshness
//!   check to forget: unforgeable and non-`Clone` stopped duplication, but
//!   neither stopped *holding* — a clearance minted at `sync setup` could be
//!   moved across any interval into a `fn push(clearance: PushClearance)` that
//!   never looked at the clock (security finding F-3, whose `assert_fresh` had
//!   zero production callers and read as wired because it had tests).
//!
//! **The contract for Phase 4's write path:** every verb that sends a byte takes
//! a [`Pushing`] by value. A `Pushing` comes only from
//! `assert_pushable(…)?.0.spend(now)?`, so the sequence
//! [`fetch_facts`] → [`assert_pushable`] → [`PushClearance::spend`] runs inside
//! the push, within [`MAX_CLEARANCE_AGE`] of the first byte — never carried over
//! from `sync setup`, which no longer hands one out at all.
//!
//! Plan 3-04 filled [`assert_pushable`]'s remaining refusal conditions and its
//! warning cases behind the signature frozen here, and added the REPO-03 guard
//! test at the bottom of this file — the standing check that no
//! repository-creating endpoint is reachable from anywhere under `src/`.

use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;

use crate::error::{AppError, Result};

use super::http;
use super::{Client, RepoRef};

/// How stale a [`PushClearance`] may be at the moment of a push.
///
/// Small on purpose: the window between the gate and the first byte is a few
/// HTTP round trips, and every second of slack is a second in which the
/// repository could have been flipped public.
pub const MAX_CLEARANCE_AGE: Duration = Duration::from_secs(30);

/// What `GET /repos/{owner}/{name}` says about the repository.
///
/// `private` alone is not enough: an *internal* repository on an enterprise
/// account also reports `private: true`, so `visibility` is carried too and
/// plan 3-04 asserts both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoFacts {
    pub id: u64,
    pub private: bool,
    pub visibility: String,
    pub owner_login: String,
    /// The numeric owner id — what makes a delete-and-resquat of the repository
    /// *name* detectable. A login comparison alone would not see it.
    pub owner_id: u64,
    pub archived: bool,
    pub fork: bool,
    /// `permissions.admin`, absent for a fine-grained PAT without it. D-03 warns
    /// rather than fails when it is present: a token that could create a public
    /// repository silently weakens REPO-03's structural guarantee.
    pub admin_permission: bool,
}

/// The wire shape, kept separate so [`RepoFacts`] is flat for every consumer and
/// so a missing `permissions` object is a `false`, never a parse failure.
#[derive(Deserialize)]
struct RawRepo {
    id: u64,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    visibility: String,
    owner: RawOwner,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    fork: bool,
    #[serde(default)]
    permissions: Option<RawPermissions>,
}

#[derive(Deserialize)]
struct RawOwner {
    login: String,
    id: u64,
}

#[derive(Deserialize, Default)]
struct RawPermissions {
    #[serde(default)]
    admin: bool,
}

/// Why [`fetch_facts`] failed, carrying the one distinction `AppError` cannot
/// make.
///
/// [`AppError::Credentials`] covers **two** unrelated things on this path:
/// GitHub rejecting the token, and `Client::get_json` finding the token is not a
/// legal HTTP header value. Only the first means the stored value is dead.
/// Classifying on the `AppError` arm rather than on the status is what let a
/// token with one illegal byte silently delete a Keychain item (F-1), so the
/// answer travels as a field rather than being re-derived from a message.
#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct FetchError {
    pub error: AppError,
    /// True **only** for [`http::GithubError::Unauthorized`] — a 401 from
    /// GitHub, about the value this request actually sent.
    pub token_rejected: bool,
}

impl FetchError {
    /// Everything that is not a 401: transport, a malformed token, a body that
    /// is not a repository, D-01's 404 text.
    fn local(error: AppError) -> Self {
        FetchError {
            error,
            token_rejected: false,
        }
    }
}

impl From<FetchError> for AppError {
    fn from(err: FetchError) -> Self {
        err.error
    }
}

/// `GET /repos/{owner}/{name}` — the only request the gate makes.
///
/// A 404 becomes D-01's message: GitHub returns the same status for "no such
/// repository" and "your token is not scoped to it", so the message says both,
/// and names the command that creates one. This tool never creates a repository
/// (REPO-03); a 404 that auto-created one would also happily create it for a
/// name squatter.
pub async fn fetch_facts(
    client: &Client,
    repo: &RepoRef,
    now: DateTime<Utc>,
) -> std::result::Result<RepoFacts, FetchError> {
    let (status, headers, body) = client
        .get_json(&format!("/repos/{}/{}", repo.owner, repo.name))
        .await
        .map_err(FetchError::local)?;

    if !status.is_success() {
        let err = http::classify(status, &headers, &body, now);
        return Err(match err {
            http::GithubError::NotFound { .. } => {
                FetchError::local(AppError::Other(missing_repo_message(repo)))
            }
            // The one status that says *this token* is dead, and the only one
            // that may clear anything.
            unauthorized @ http::GithubError::Unauthorized { .. } => FetchError {
                error: AppError::from(unauthorized),
                token_rejected: true,
            },
            other => FetchError::local(AppError::from(other)),
        });
    }

    let raw: RawRepo = serde_json::from_slice(&body).map_err(|e| {
        FetchError::local(AppError::Schema(format!(
            "GitHub's description of {repo} was not the shape this build expects ({e})"
        )))
    })?;
    Ok(RepoFacts {
        id: raw.id,
        private: raw.private,
        visibility: raw.visibility,
        owner_login: raw.owner.login,
        owner_id: raw.owner.id,
        archived: raw.archived,
        fork: raw.fork,
        admin_permission: raw.permissions.unwrap_or_default().admin,
    })
}

/// D-01's 404 text, `pub(crate)` so every entry point that can produce a
/// "there is no such repository" answer says the same thing — including the
/// ones plan 3-07 adds. GitHub deliberately 404s an unauthorised private
/// repository exactly as it 404s a missing one, so the message names both and
/// then names the command that fixes either.
pub(crate) fn missing_repo_message(repo: &RepoRef) -> String {
    format!(
        "{repo} is not there. Either it does not exist, or this token is not scoped to it — \
         GitHub returns the same 404 for both, deliberately, so both are possible.\n\
         ai-usagebar never creates a repository. Create it yourself, private:\n\
         \x20   gh repo create {repo} --private"
    )
}

/// The only `visibility` this gate accepts. GitHub reports exactly one of
/// `"public"`, `"private"`, or `"internal"`.
const VISIBILITY_PRIVATE: &str = "private";
/// Called out by name rather than caught by a catch-all: an internal repository
/// reports `private: true`, so it is the one value that slips past a `private`
/// check, and its refusal has to explain why.
const VISIBILITY_INTERNAL: &str = "internal";

/// Proof that a private-repo check passed, and when.
///
/// Private field, no public constructor, no `Clone`, no `Copy`: the only way to
/// hold one is to have just run [`assert_pushable`]. That is the whole design —
/// a clearance that could be stashed and duplicated *is* a cached check, which
/// is exactly what D-04 forbids.
#[derive(Debug)]
#[must_use = "a clearance that is not spent is a private-repo check nothing acted on"]
pub struct PushClearance {
    checked_at: DateTime<Utc>,
    repo: RepoRef,
}

/// Permission to send a byte **to one named repository**, and the only thing
/// that carries it.
///
/// Private fields, no public constructor, no `Clone`: the sole way to hold one
/// is [`PushClearance::spend`], which does the freshness arithmetic on the way
/// through. A write verb that takes a `Pushing` therefore cannot be reached
/// without a check that was fresh at the call — not by discipline, and not by a
/// method someone remembers to invoke.
///
/// # Why it names its subject
///
/// A permit proving only *when* a check happened proves nothing about *what* it
/// was about: one minted against a private repo A would type-check against a
/// write to repo B, and the design would hold only because every caller happens
/// to thread the same `ctx.repo` through both. [`Pushing::covers`] makes the
/// subject structural, matching the pattern `Root`'s AAD already uses — bind the
/// identity rather than trusting that two call sites agree.
#[derive(Debug)]
#[must_use = "a Pushing is permission to upload; dropping one uploads nothing"]
pub struct Pushing(RepoRef);

impl Pushing {
    /// Refuse a write to a repository this permit was not minted against.
    ///
    /// Called by every write verb in [`write`](super::write) before the request
    /// is built, so the check cannot be skipped by adding a verb that forgets it
    /// — the permit is the only way in, and this is the only way to read it.
    pub(crate) fn covers(&self, repo: &RepoRef) -> Result<()> {
        if self.0 == *repo {
            return Ok(());
        }
        Err(AppError::Other(format!(
            "REFUSING TO WRITE: the private-repo check was run against {}, but this request \
             targets {repo}. A clearance proves one repository was private at one instant and \
             says nothing about any other.",
            self.0
        )))
    }
}

impl PushClearance {
    pub fn checked_at(&self) -> DateTime<Utc> {
        self.checked_at
    }

    /// D-04's "immediately", as arithmetic that cannot be skipped: this consumes
    /// the clearance, so spending it *is* checking it.
    ///
    /// A clearance dated in the *future* fails too: that is a clock that moved,
    /// and an unbounded-looking age is not something to wave a push through on.
    pub fn spend(self, now: DateTime<Utc>) -> Result<Pushing> {
        let age = now.signed_duration_since(self.checked_at);
        let fresh = age >= TimeDelta::zero() && age.to_std().is_ok_and(|a| a <= MAX_CLEARANCE_AGE);
        if fresh {
            return Ok(Pushing(self.repo));
        }
        Err(AppError::Other(format!(
            "the private-repo check is {}s old (limit {}s) — re-run it immediately before \
             pushing, because the repository can be made public at any moment",
            age.num_seconds(),
            MAX_CLEARANCE_AGE.as_secs()
        )))
    }
}

/// The gate. **Signature frozen by plan 3-01, and byte-identical to it.**
///
/// `credentials_in_bundle` is D-04 read exactly as written: a public repository
/// aborts *when credentials are in the bundle*, and is allowed-with-a-warning
/// when the credentials category is off. Without the parameter this function and
/// [`pairing::check_drift`](super::pairing::check_drift) would contradict each
/// other — one hard-refusing on `private == false` while knowing nothing about
/// the bundle, the other carving out the credentials-off case — and since
/// `setup.rs` calls `check_drift` and then this, the second would silently kill
/// the first's carve-out. One function decides, and it is this one.
///
/// Six conditions refuse, each with its own message, because "the gate failed"
/// tells a user nothing they can act on:
///
/// | condition | why it is not merely paranoia |
/// |---|---|
/// | `private == false` + credentials | D-04. Names rotation, because an *earlier* push may already have landed while it was public |
/// | `visibility == "internal"` | Reports `private: true` and is readable by the whole enterprise — the one value a `private` check cannot see |
/// | `owner_login` ≠ the configured owner | GitHub follows renames and transfers silently; the name can end up pointing elsewhere |
/// | `archived` | Every write is rejected by GitHub, so this fails later at a worse moment |
/// | `fork` | A fork shares its upstream's object network, which reaches contents the owner did not intend to share |
/// | 404 | [`missing_repo_message`], from [`fetch_facts`] |
///
/// The warning list is the tuple's second element, so warning cases arrive
/// without a signature change and `setup.rs` — a file plan 3-04 does not own —
/// keeps rendering them unchanged.
pub fn assert_pushable(
    facts: &RepoFacts,
    repo: &RepoRef,
    credentials_in_bundle: bool,
    now: DateTime<Utc>,
) -> Result<(PushClearance, Vec<String>)> {
    let mut warnings: Vec<String> = Vec::new();

    // ponytail: `admin_permission` is parsed and deliberately *not* warned on,
    // and this is a reading of D-03 rather than an omission. `permissions.admin`
    // on `GET /repos/{owner}/{repo}` reports the **authenticated user's role on
    // the repository**, not the token's granted permissions — and D-01 has the
    // user create the repository themselves, which makes them its admin. So a
    // correctly-scoped `Contents: read/write` token would still see
    // `admin: true`, the warning would fire on essentially every legitimate
    // install, and a warning that always fires trains its reader to ignore it.
    // Whether a fine-grained PAT narrows the field is undocumented; plan 3-06's
    // `#[ignore]`d probe measures it against a real token. Turning the warning
    // on is one line once that measurement exists. D-03's real force is the
    // token recipe in `docs/sync-github.md`, which is what actually determines
    // the token's scope.
    let _ = facts.admin_permission;

    // D-04, and the only place the credentials carve-out is decided.
    if !facts.private {
        if credentials_in_bundle {
            // The rotation advice is not politeness. A *previous* push may have
            // landed while the repository was public, and nothing here can
            // un-publish those bytes.
            return Err(AppError::Other(format!(
                "REFUSING TO PUSH: {repo} is public (visibility {:?}).\n\
                 The bundle carries the credentials category, so nothing is uploaded.\n\
                 Make the repository private again, then rotate every credential it may \
                 already hold — an earlier push could have landed while it was public, and \
                 bytes that were published cannot be un-published.",
                facts.visibility
            )));
        }
        // D-04's closing paragraph: with the credentials category off there is
        // nothing to rotate, so this warns and proceeds rather than refusing.
        warnings.push(format!(
            "{repo} is public. The credentials category is off, so there is nothing to \
             rotate — but chat indexes and config are personal data too, and anything \
             pushed to a public repository is readable by anyone. Make it private."
        ));
    }

    // `private: true` is not the same thing as "private": an *internal*
    // repository on an enterprise account reports `private: true` and is
    // readable by every member of that enterprise. Matched by name so the
    // refusal states the reason rather than falling through a catch-all.
    if facts.private && facts.visibility != VISIBILITY_PRIVATE {
        return Err(AppError::Other(
            if facts.visibility == VISIBILITY_INTERNAL {
                format!(
                    "REFUSING TO PUSH: {repo} is an internal repository.\n\
                 An internal repository reports private: true, but every member of the \
                 enterprise that owns it can read it — that is not private enough to hold \
                 credentials. Change its visibility to private."
                )
            } else {
                format!(
                    "REFUSING TO PUSH: {repo} reports visibility {:?}, which this build does not \
                 recognise. Only \"private\" is accepted. Set the repository to private, or \
                 update ai-usagebar if GitHub has added a visibility since this release.",
                    facts.visibility
                )
            },
        ));
    }

    // GitHub follows a rename or a transfer silently, answering for the new
    // owner at the old path. The repository this answered for is then not the
    // one `[sync] repo` names. Owner names are case-insensitive on GitHub.
    if !facts.owner_login.eq_ignore_ascii_case(&repo.owner) {
        return Err(AppError::Other(format!(
            "REFUSING TO PUSH: {repo} is owned by {:?}, not {:?}.\n\
             GitHub answers for a renamed or transferred repository at its old path, so the \
             name in [sync] repo can quietly end up pointing somewhere else. Point [sync] \
             repo at the repository you mean, then re-run `ai-usagebar sync setup`.",
            facts.owner_login, repo.owner
        )));
    }

    if facts.archived {
        return Err(AppError::Other(format!(
            "REFUSING TO PUSH: {repo} is archived.\n\
             GitHub rejects every write to an archived repository, so a push would fail \
             later — mid-upload, with a partly-written snapshot. Unarchive it in the \
             repository's settings, then re-run."
        )));
    }

    if facts.fork {
        return Err(AppError::Other(format!(
            "REFUSING TO PUSH: {repo} is a fork.\n\
             A fork shares its upstream's object network, which can make its contents \
             reachable in ways the owner did not intend. Use a repository created on its \
             own, not one forked from another."
        )));
    }

    Ok((
        PushClearance {
            checked_at: now,
            repo: repo.clone(),
        },
        warnings,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::github::Endpoints;
    use crate::sync::github::token::TokenSource;
    use zeroize::Zeroizing;

    const PRIVATE_BODY: &str = r#"{"id":1,"private":true,"visibility":"private",
        "owner":{"login":"o","id":7},"archived":false,"fork":false}"#;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn repo() -> RepoRef {
        RepoRef::parse("o/n").unwrap()
    }

    fn client_at(base: &str) -> Client {
        Client::new(
            &Endpoints {
                api_base: base.into(),
                uploads_base: base.into(),
            },
            Zeroizing::new("github_pat_fixture_not_a_real_token".into()),
            TokenSource::Env,
        )
        .unwrap()
    }

    /// One valid fact set with a single field bent, so each test names exactly
    /// the condition it is about.
    fn with(bend: impl FnOnce(&mut RepoFacts)) -> RepoFacts {
        let mut f = facts(true);
        bend(&mut f);
        f
    }

    /// `private: true` *and* invisible to the `private` check — the shape the
    /// `visibility` field exists to catch.
    fn internal() -> RepoFacts {
        with(|f| f.visibility = VISIBILITY_INTERNAL.into())
    }

    fn facts(private: bool) -> RepoFacts {
        RepoFacts {
            id: 1,
            private,
            visibility: if private { "private" } else { "public" }.into(),
            owner_login: "o".into(),
            owner_id: 7,
            archived: false,
            fork: false,
            admin_permission: false,
        }
    }

    #[tokio::test]
    async fn a_private_repository_yields_the_five_facts_the_gate_asserts_on() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body(PRIVATE_BODY)
            .create_async()
            .await;

        let got = fetch_facts(&client_at(&server.url()), &repo(), now())
            .await
            .unwrap();
        assert_eq!(got, facts(true));
        m.assert_async().await;
    }

    /// `permissions.admin` is D-03's warning input, and the object is absent for
    /// a correctly-scoped fine-grained PAT — absence must not be a parse error.
    #[tokio::test]
    async fn an_admin_permission_is_read_when_present_and_false_when_absent() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body(
                r#"{"id":1,"private":true,"visibility":"private","owner":{"login":"o","id":7},
                    "archived":false,"fork":false,"permissions":{"admin":true,"push":true}}"#,
            )
            .create_async()
            .await;

        let got = fetch_facts(&client_at(&server.url()), &repo(), now())
            .await
            .unwrap();
        assert!(got.admin_permission);
    }

    /// F-1's trigger, at the source: only a 401 sets `token_rejected`, and it is
    /// the *only* thing `setup::clear_if_dead` may act on. The 403 next to it is
    /// the one a wrong predicate would have cleared a working token for.
    #[tokio::test]
    async fn only_a_401_marks_the_token_as_rejected() {
        for (status, rejected) in [(401, true), (403, false), (500, false)] {
            let mut server = mockito::Server::new_async().await;
            let _m = server
                .mock("GET", "/repos/o/n")
                .with_status(status)
                .with_body(r#"{"message":"whatever"}"#)
                .create_async()
                .await;

            let err = fetch_facts(&client_at(&server.url()), &repo(), now())
                .await
                .expect_err("not a repository description");
            assert_eq!(err.token_rejected, rejected, "HTTP {status}: {err}");
        }
        // …and a transport failure, which never got a status at all.
        let err = fetch_facts(&client_at("http://127.0.0.1:1"), &repo(), now())
            .await
            .expect_err("nothing listens there");
        assert!(!err.token_rejected, "{err}");
    }

    /// D-01: GitHub 404s an unauthorised private repository exactly as it 404s a
    /// missing one, so the message says both and names the fix.
    #[tokio::test]
    async fn a_404_names_both_causes_and_prints_the_create_command() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/o/n")
            .with_status(404)
            .with_body(r#"{"message":"Not Found"}"#)
            .create_async()
            .await;

        let err = fetch_facts(&client_at(&server.url()), &repo(), now())
            .await
            .expect_err("a 404 is not a repository");
        let text = err.to_string();
        assert!(text.contains("gh repo create o/n --private"), "{text}");
        assert!(text.contains("not scoped to it"), "{text}");
        assert!(text.contains("never creates a repository"), "{text}");
    }

    #[tokio::test]
    async fn a_body_that_is_not_a_repository_description_is_a_schema_failure() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body("[]")
            .create_async()
            .await;

        let err = fetch_facts(&client_at(&server.url()), &repo(), now())
            .await
            .expect_err("that is not a repository");
        assert!(matches!(err.error, AppError::Schema(_)), "{err}");
        assert!(!err.token_rejected, "a bad body is not a rejected token");
    }

    #[test]
    fn a_private_repository_clears_with_no_warnings_and_the_injected_clock() {
        let (clearance, warnings) = assert_pushable(&facts(true), &repo(), true, now()).unwrap();
        assert_eq!(clearance.checked_at(), now());
        assert!(warnings.is_empty());
    }

    /// SAFE-01. The credential-bearing arm has to name rotation: an earlier push
    /// may have landed while the repository was public.
    #[test]
    fn a_public_repository_refuses_and_says_to_rotate_when_credentials_are_in_the_bundle() {
        let err = assert_pushable(&facts(false), &repo(), true, now())
            .expect_err("a public repository is not pushable");
        let text = err.to_string();
        assert!(text.contains("REFUSING TO PUSH"), "{text}");
        assert!(text.contains("rotate"), "{text}");
        assert!(text.contains("cannot be un-published"), "{text}");
    }

    /// D-04's closing paragraph. With nothing to rotate the right answer is a
    /// warning, not a refusal — and the warning still has to be said, because a
    /// chat index is personal data.
    #[test]
    fn a_public_repository_warns_and_clears_when_credentials_are_not_in_the_bundle() {
        let (clearance, warnings) = assert_pushable(&facts(false), &repo(), false, now())
            .expect("with the credentials category off, public is a warning");
        assert_eq!(clearance.checked_at(), now());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("o/n is public"), "{warnings:?}");
        assert!(warnings[0].contains("chat indexes"), "{warnings:?}");

        // Distinct from the refusal, which is the one that must say "rotate".
        let refused = assert_pushable(&facts(false), &repo(), true, now())
            .expect_err("public")
            .to_string();
        assert_ne!(refused, warnings[0]);
        // It says there is *nothing* to rotate, and never the un-publishable
        // line — that one belongs to the refusal alone.
        assert!(warnings[0].contains("nothing to rotate"), "{warnings:?}");
        assert!(!warnings[0].contains("un-published"), "{warnings:?}");
    }

    /// The whole reason `visibility` is carried alongside `private`: an internal
    /// repository reports `private: true` and would otherwise walk straight
    /// through the gate.
    #[test]
    fn an_internal_repository_refuses_in_both_bundle_configurations() {
        for credentials in [true, false] {
            let err = assert_pushable(&internal(), &repo(), credentials, now())
                .expect_err("internal is not a public-repository carve-out");
            let text = err.to_string();
            assert!(text.contains("internal repository"), "{text}");
            assert!(text.contains("not private enough"), "{text}");
        }
    }

    #[test]
    fn an_unrecognised_visibility_refuses_rather_than_being_waved_through() {
        let err = assert_pushable(
            &with(|f| f.visibility = "some-future-value".into()),
            &repo(),
            true,
            now(),
        )
        .expect_err("only the private literal is accepted");
        let text = err.to_string();
        assert!(text.contains("some-future-value"), "{text}");
        assert!(text.contains("does not recognise"), "{text}");
    }

    #[test]
    fn a_renamed_or_transferred_owner_refuses_and_case_does_not_matter() {
        let err = assert_pushable(
            &with(|f| f.owner_login = "someone-else".into()),
            &repo(),
            true,
            now(),
        )
        .expect_err("that is not the repository [sync] repo names");
        let text = err.to_string();
        assert!(text.contains("someone-else"), "{text}");
        assert!(text.contains("[sync] repo"), "{text}");

        // GitHub owner names are case-insensitive; a differing case is the same
        // account and must not refuse.
        assert!(
            assert_pushable(&with(|f| f.owner_login = "O".into()), &repo(), true, now()).is_ok()
        );
    }

    #[test]
    fn an_archived_repository_refuses_before_the_write_that_would_fail_later() {
        let err = assert_pushable(&with(|f| f.archived = true), &repo(), true, now())
            .expect_err("GitHub rejects every write to an archived repository");
        assert!(err.to_string().contains("archived"), "{err}");
    }

    #[test]
    fn a_fork_refuses_because_it_shares_its_upstreams_object_network() {
        let err = assert_pushable(&with(|f| f.fork = true), &repo(), true, now())
            .expect_err("a fork's contents are reachable from its upstream");
        assert!(err.to_string().contains("fork"), "{err}");
    }

    /// The phase's first success criterion: each refusal is actionable on its
    /// own, so none of them may share a message with another.
    #[test]
    fn all_six_refusals_say_six_different_things() {
        let messages = [
            assert_pushable(&facts(false), &repo(), true, now())
                .unwrap_err()
                .to_string(),
            assert_pushable(&internal(), &repo(), true, now())
                .unwrap_err()
                .to_string(),
            assert_pushable(
                &with(|f| f.owner_login = "elsewhere".into()),
                &repo(),
                true,
                now(),
            )
            .unwrap_err()
            .to_string(),
            assert_pushable(&with(|f| f.archived = true), &repo(), true, now())
                .unwrap_err()
                .to_string(),
            assert_pushable(&with(|f| f.fork = true), &repo(), true, now())
                .unwrap_err()
                .to_string(),
            missing_repo_message(&repo()),
        ];
        let unique: std::collections::BTreeSet<&String> = messages.iter().collect();
        assert_eq!(unique.len(), messages.len(), "{messages:#?}");
    }

    /// D-03. The field is read from the wire and deliberately produces no
    /// warning — see the comment in `assert_pushable`, and plan 3-06's probe.
    #[test]
    fn an_administrative_permission_produces_no_warning_yet() {
        let (_, warnings) =
            assert_pushable(&with(|f| f.admin_permission = true), &repo(), true, now()).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// D-01, asserted on the message itself rather than only through a mock, so
    /// every caller of it inherits the same coverage.
    #[test]
    fn the_missing_repository_message_names_both_causes_and_substitutes_the_repo() {
        let text = missing_repo_message(&RepoRef::parse("acme/backups").unwrap());
        assert!(text.contains("does not exist"), "{text}");
        assert!(text.contains("not scoped to it"), "{text}");
        assert!(
            text.contains("gh repo create acme/backups --private"),
            "{text}"
        );
    }

    /// REPO-03, as a standing check rather than a one-time grep.
    ///
    /// Withholding `Administration: write` is what makes creating a repository —
    /// and therefore a **public** one — impossible rather than merely
    /// disallowed. A `grep` in a plan's verify step proves that for one
    /// afternoon; REPO-03 is a property of the shipped crate, so this walks
    /// `src/` on every `cargo test`, and therefore on every `make test`, and
    /// therefore in every later phase. The cost is one directory walk.
    ///
    /// All four creating endpoints, not just the obvious one:
    ///
    /// - the user-namespace create path,
    /// - the organization-namespace create path — D-01 explicitly permits an
    ///   organization owner, so this is a live route and not a hypothetical,
    /// - create-from-template,
    /// - **fork**, which is the dangerous one: a fork of a public upstream is
    ///   public, which is exactly the outcome REPO-03 exists to make impossible.
    ///
    /// The organization fragment is deliberately broader than its create path.
    /// This crate calls no organization endpoint at all, so matching every one
    /// of them costs nothing and catches every spelling of the create path,
    /// including ones a `format!` would break into pieces.
    ///
    /// **This file is excluded from the walk, deliberately.** The fragments
    /// below are the things being searched for, so a guard that scanned its own
    /// source would fail on the day it was written. That exclusion is also the
    /// rule for everyone else: do not write any of these fragments anywhere
    /// under `src/` — not in a call, not in a test fixture, and not in a comment
    /// explaining that the endpoint is never used. To this test a comment and a
    /// call site are indistinguishable. Say what is true instead: the tool
    /// refuses and prints the command the user should run.
    ///
    /// The braces to plan 3-01's belt, which is the stronger half: `Client`
    /// exposes no method that can carry a request body, and none of these four
    /// endpoints is reachable without one. Both are cheap; having both is right.
    #[test]
    fn no_repository_creating_endpoint_is_reachable_from_the_crate() {
        const FORBIDDEN: [&str; 4] = ["/user/repos", "/orgs/", "/generate", "/forks"];

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let files = crate::sync::guard::rs_files(&root);

        let mut scanned = 0usize;
        let mut skipped = 0usize;
        for path in &files {
            if path.ends_with(file!()) {
                skipped += 1;
                continue;
            }
            scanned += 1;
            let text = std::fs::read_to_string(path).unwrap();
            for fragment in FORBIDDEN {
                assert!(
                    !text.contains(fragment),
                    "REPO-03: {} contains {fragment:?}. ai-usagebar holds no permission that \
                     could create a repository, and that is a structural guarantee rather \
                     than a policy — creating one is how a *public* repository comes to \
                     exist. Remove the path. If a repository is missing, print the \
                     `gh repo create <owner>/<name> --private` line and exit non-zero.",
                    path.display()
                );
            }
        }

        // Non-vacuity: a guard that asserts an absence must also prove it looked
        // at something. A refactor that moved this file, or a walk that silently
        // found nothing, would otherwise report green forever.
        assert_eq!(skipped, 1, "this file must be excluded exactly once");
        assert!(scanned > 50, "only {scanned} files walked under {root:?}");
    }

    /// D-04's "immediately", enforceable rather than merely described — and
    /// spending is the *only* way to reach a [`Pushing`], so it cannot be
    /// skipped by a caller who simply never calls it (F-3).
    ///
    /// A fresh clearance per case: `spend` consumes it, which is the point.
    #[test]
    fn a_clearance_goes_stale_and_a_clearance_from_the_future_is_refused() {
        let minted = || {
            assert_pushable(&facts(true), &repo(), true, now())
                .unwrap()
                .0
        };

        assert!(minted().spend(now()).is_ok());
        assert!(minted().spend(now() + TimeDelta::seconds(5)).is_ok());

        let stale = minted()
            .spend(now() + TimeDelta::seconds(31))
            .expect_err("31s is past the 30s limit");
        assert!(stale.to_string().contains("immediately before"), "{stale}");
        assert!(
            stale
                .to_string()
                .contains(&MAX_CLEARANCE_AGE.as_secs().to_string()),
            "{stale}"
        );

        assert!(
            minted().spend(now() - TimeDelta::seconds(1)).is_err(),
            "a clearance dated in the future is a clock that moved"
        );
    }

    /// F-3, as a standing check rather than a claim in a doc comment: nothing
    /// but `spend` may hand out a `Pushing`, and `spend` is the only `&self`-free
    /// exit from a `PushClearance`. A re-added `assert_fresh`-shaped method —
    /// one that reports freshness without consuming the capability — puts the
    /// forgettable path back, so the name is refused outright.
    #[test]
    fn freshness_is_the_only_exit_from_a_clearance() {
        let source =
            std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file!()))
                .unwrap();
        let code = crate::sync::guard::production_code(&source);
        assert!(
            code.contains("pub fn spend(self,"),
            "spend must consume self"
        );
        assert!(
            !code.contains("fn assert_fresh"),
            "a &self freshness check is exactly the thing that had no callers"
        );
        // `Pushing`'s field is private, so `Pushing(..)` outside this module
        // does not compile; the only construction is inside `spend`.
        assert_eq!(code.matches("Ok(Pushing(").count(), 1, "one mint site");
        assert_eq!(
            code.matches("Pushing(").count(),
            2,
            "the declaration and the one mint site, and nothing else"
        );
    }

    /// **F-6.** A permit proves *when* the check happened and *what it was
    /// about*. Without the second half a clearance minted against a private repo
    /// A type-checks against a write to repo B, and the design holds only
    /// because every caller happens to thread one `ctx.repo` through both.
    #[test]
    fn a_permit_minted_for_one_repository_refuses_a_write_to_another() {
        let permit = assert_pushable(&facts(true), &repo(), true, now())
            .unwrap()
            .0
            .spend(now())
            .unwrap();

        assert!(permit.covers(&repo()).is_ok());

        let elsewhere = RepoRef::parse("o/other").unwrap();
        let err = permit
            .covers(&elsewhere)
            .expect_err("a permit is not transferable between repositories")
            .to_string();
        assert!(err.contains("o/n") && err.contains("o/other"), "{err}");

        // Every write verb in `write.rs` consults it, so the check cannot be
        // skipped by adding a verb that forgets to.
        let write = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sync/github/write.rs"),
        )
        .unwrap();
        let code = crate::sync::guard::production_code(&write);
        assert_eq!(
            code.matches("permit.covers(repo)?;").count(),
            code.matches("permit: &Pushing,").count(),
            "every verb taking a Pushing must check that it covers this repo"
        );
        assert!(
            code.matches("permit: &Pushing,").count() >= 4,
            "the four write verbs are still there"
        );
    }
}
