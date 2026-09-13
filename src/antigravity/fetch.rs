//! Fetch a Google Antigravity usage snapshot from a local language server.
//!
//! Google ships three separate Antigravity products — Antigravity 2.0, the
//! `agy` CLI, and the Antigravity IDE — and they all draw on the **same**
//! account-wide quota. A machine may have any combination of them installed and
//! running, so this module probes every local server it can find and trusts the
//! first that answers; there is no need to prefer one product over another.
//!
//! Each exposes a CSRF-guarded JSON-RPC surface on a **dynamically assigned**
//! loopback port (`--https_server_port 0`), so the port cannot be hardcoded.
//! Quota lives behind `RetrieveUserQuotaSummary`, which reports two model groups
//! — Gemini, and Claude/GPT — each holding a 5-hour and a weekly bucket.
//! `GetUserStatus` carries only the plan name; its per-model `quotaInfo` mirrors
//! whichever bucket is scarcest and must not be read as a window in its own
//! right.
//!
//! When no usable product is running there is still a way to answer:
//! Antigravity keeps the Google session it signed in with in the OS keyring,
//! and the same quota summary is served by the Cloud Code API. That is the
//! *fallback*, taken when no local server was found or when `agy` reports that
//! its undiscoverable CSRF token is required. Every other local rejection — a
//! server that is signed out or answering on the wrong protocol — keeps its
//! own diagnosis.

use std::time::Duration;

use chrono::{DateTime, Utc};

use super::cloud;
use super::credential::{self, StoredToken};
use crate::cache::{Cache, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::{AntigravitySnapshot, AntigravitySource, UsageWindow};

const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

const QUOTA_RPC: &str = "exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const STATUS_RPC: &str = "exa.language_server_pb.LanguageServerService/GetUserStatus";

const DEFAULT_PLAN: &str = "Antigravity";

const NO_LOCAL_SERVER: &str = "Antigravity: no local server found. Quota is only served while \
                               Antigravity is running — open the Antigravity app, or an interactive \
                               `agy` session, or point ANTIGRAVITY_LS_ADDRESS at a host:port.";

const AGY_CSRF_UNAVAILABLE: &str = "Antigravity: the running `agy` server requires a CSRF token \
                                   that it does not publish.";

/// Appended to [`NO_LOCAL_SERVER`] once the remote fallback has also come up
/// empty: the user has a second way out that the local-only message does not
/// mention.
const NO_SAVED_SESSION: &str = "Or sign in to Antigravity once, so its saved Google session can \
                                be used while it is closed.";

const SESSION_EXPIRED: &str =
    "Antigravity's saved Google session expired; open Antigravity to sign in again";

/// This vendor's [`Outcome`](crate::outcome::Outcome) — the shared shape,
/// specialised to its snapshot.
pub type FetchOutcome = crate::outcome::Outcome<AntigravitySnapshot>;

impl From<FetchOutcome> for crate::vendor::VendorOutcome {
    fn from(o: FetchOutcome) -> Self {
        o.map(crate::usage::VendorSnapshot::Antigravity)
    }
}

/// The keyring blob, or a stand-in for it. `Absent` exists so a test can
/// exercise "nothing saved" without asking the real keyring, which is what
/// `None` would have to mean otherwise.
#[derive(Debug, Clone, Copy, Default)]
pub enum SavedCredential<'a> {
    /// Read the OS keyring, as production does.
    #[default]
    Keyring,
    /// Use this raw blob.
    Blob(&'a str),
    /// Behave as if the keyring held nothing.
    Absent,
}

/// Test seam for the remote fallback. Production passes
/// [`RemoteOverride::default`], which reads the OS keyring, talks to Google,
/// and probes the local ports discovery finds; a test supplies each of those
/// instead so it never touches the real keyring, the network, or `/proc`.
#[derive(Default)]
pub struct RemoteOverride<'a> {
    /// Where the saved Google session comes from.
    pub credential: SavedCredential<'a>,
    /// Cloud Code endpoints, in place of [`cloud::Endpoints::default`].
    pub endpoints: Option<&'a cloud::Endpoints>,
    /// Local base URLs to probe, in place of discovery. `Some(vec![])` means
    /// "no local server", which is what sends the fetch down the remote path.
    pub local_bases: Option<Vec<String>>,
}

pub async fn fetch_snapshot(
    client: &reqwest::Client,
    cache: &Cache,
    cache_ttl: Duration,
    oauth: Option<&cloud::OauthClient>,
) -> Result<FetchOutcome> {
    fetch_snapshot_at(
        client,
        cache,
        cache_ttl,
        oauth,
        RemoteOverride::default(),
        Utc::now(),
    )
    .await
}

/// Clock seam for [`fetch_snapshot`], so window expiry can be exercised at
/// fixed instants instead of against the wall clock, and the seam for
/// everything the remote fallback would otherwise read from the machine.
pub async fn fetch_snapshot_at(
    client: &reqwest::Client,
    cache: &Cache,
    cache_ttl: Duration,
    oauth: Option<&cloud::OauthClient>,
    remote: RemoteOverride<'_>,
    now: DateTime<Utc>,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;

    // Resolve the signed-in account first so a fresh cache can be attributed.
    // Unlike Grok — where the same check would cost a remote round-trip on
    // every poll — this is loopback, and it is the call that would supply the
    // plan name anyway, so verification is effectively free.
    //
    // With no local server at all, the saved Google session identifies the
    // account just as cheaply: reading the keyring is local, and only the
    // quota call itself goes to the network — after the fresh-cache check,
    // like the local RPC.
    let origin = match open_session(client, remote.local_bases.as_deref()).await {
        Ok(session) => Origin::Local(Ok(session)),
        Err(error) => match remote_fallback_reason(&error) {
            Some(reason) => Origin::Remote(saved_session(remote.credential, reason)),
            None => Origin::Local(Err(error)),
        },
    };
    let account = origin.account();

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse_cache(bytes, cache, false, account.as_deref(), now)
    {
        return Ok(outcome);
    }

    let default_endpoints = cloud::Endpoints::default();
    let endpoints = remote.endpoints.unwrap_or(&default_endpoints);
    let live = match origin {
        Origin::Local(session) => fetch_live(client, session).await,
        Origin::Remote(token) => fetch_remote(client, cache, oauth, endpoints, token, now).await,
    };

    match live {
        Ok(snap) => {
            let bytes = serde_json::to_vec(&snap_to_json(&snap))?;
            cache.write_payload(&bytes)?;
            Ok(crate::outcome::Outcome::fresh(snap))
        }
        Err(e) if e.is_transient() => fallback_silent(cache, now, e),
        Err(AppError::Http { status, body }) => {
            cache.mark_stale();
            let last_error = Some(cache.write_last_error(status, &body));
            let reason = AppError::Http { status, body };
            fallback_with_error(cache, last_error, reason, now)
        }
        Err(e) => {
            cache.mark_stale();
            let last_error = Some(cache.write_last_error(0, &e.to_string()));
            fallback_with_error(cache, last_error, e, now)
        }
    }
}

/// A local server that answered `GetUserStatus`: where it lives, how to talk to
/// it, and whose account it is signed in as.
struct Session {
    base: String,
    csrf: Option<String>,
    plan: String,
    account: String,
}

/// Which source this fetch will draw on, decided before the cache is consulted
/// so a fresh payload can be checked against the right account.
enum Origin {
    Local(Result<Session>),
    Remote(Result<StoredToken>),
}

impl Origin {
    /// The account fingerprint, when the source identified one.
    fn account(&self) -> Option<String> {
        match self {
            Origin::Local(Ok(session)) => Some(session.account.clone()),
            Origin::Remote(Ok(token)) => Some(remote_account(&token.fingerprint)),
            Origin::Local(Err(_)) | Origin::Remote(Err(_)) => None,
        }
    }
}

fn no_local_server() -> AppError {
    AppError::Credentials(NO_LOCAL_SERVER.into())
}

#[derive(Clone, Copy)]
enum RemoteFallbackReason {
    NoLocalServer,
    AgyMissingCsrf,
}

impl RemoteFallbackReason {
    fn message(self) -> &'static str {
        match self {
            Self::NoLocalServer => NO_LOCAL_SERVER,
            Self::AgyMissingCsrf => AGY_CSRF_UNAVAILABLE,
        }
    }
}

/// Local failures the saved Google session is allowed to answer. The `agy`
/// response is matched structurally and exactly; an arbitrary local `401`
/// remains a signed-out diagnosis and never triggers remote traffic.
fn remote_fallback_reason(error: &AppError) -> Option<RemoteFallbackReason> {
    if matches!(error, AppError::Credentials(message) if message == NO_LOCAL_SERVER) {
        Some(RemoteFallbackReason::NoLocalServer)
    } else if is_missing_csrf(error) {
        Some(RemoteFallbackReason::AgyMissingCsrf)
    } else {
        None
    }
}

/// Walk every candidate language server until one identifies itself. A machine
/// can host more than one — the desktop app, the IDE and an interactive `agy`
/// session each run their own — and only some of them are signed in.
///
/// `bases` replaces discovery when given; see [`RemoteOverride::local_bases`].
async fn open_session(client: &reqwest::Client, bases: Option<&[String]>) -> Result<Session> {
    let bases = bases.map_or_else(candidate_bases, <[String]>::to_vec);
    if bases.is_empty() {
        return Err(no_local_server());
    }

    let mut errors = Vec::new();
    for base in bases {
        let csrf = fetch_csrf(client, &base).await;
        match post_rpc(client, &base, csrf.as_deref(), STATUS_RPC).await {
            Ok(v) => {
                return Ok(Session {
                    base,
                    csrf,
                    plan: plan_from_status(&v),
                    account: account_key(&v),
                });
            }
            Err(e) => errors.push(e),
        }
    }
    Err(select_probe_error(errors))
}

/// Which failure to report when no candidate answered.
///
/// A server that replies `401`/`403` is normally running and reachable but
/// signed out — the user can act on that, so it outranks the connection
/// refusals from products that simply are not up. `agy`'s exact missing-CSRF
/// response is ranked separately because it has no token discovery route.
/// Without this, a stale
/// `ANTIGRAVITY_LS_ADDRESS` (or a second product on another port) would mask
/// the one message worth reading behind transport noise.
///
/// Note that this also decides *visibility*: transport errors are transient and
/// fall back silently to cache, while the `401` surfaces in the widget. That is
/// why a TLS listener's echo is ranked below everything else rather than merely
/// tie-breaking — see [`is_tls_echo`]. Being an `Http`, it is not transient, so
/// letting it stand as "the last failure" turns a product that simply is not
/// serving RPC into a visible error about a protocol the user never chose.
fn select_probe_error(errors: Vec<AppError>) -> AppError {
    let mut actionable = None;
    let mut missing_csrf = None;
    let mut last = None;
    let mut echo = None;
    for e in errors {
        if missing_csrf.is_none() && is_missing_csrf(&e) {
            missing_csrf = Some(e);
        } else if actionable.is_none() && is_actionable(&e) {
            actionable = Some(e);
        } else if is_tls_echo(&e) {
            echo = Some(e);
        } else {
            last = Some(e);
        }
    }
    actionable
        .or(missing_csrf)
        .or(last)
        .or(echo)
        .unwrap_or_else(|| {
            AppError::Other("antigravity: no local server answered GetUserStatus".into())
        })
}

/// `agy` currently serves no page containing its CSRF token, then returns this
/// structured response from the status RPC. Matching the status, code, and
/// message avoids treating an unrelated local service or a genuinely
/// signed-out Antigravity product as permission to use the cloud fallback.
fn is_missing_csrf(error: &AppError) -> bool {
    let AppError::Http { status: 401, body } = error else {
        return false;
    };
    let Ok(body) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    matches!(
        (body["code"].as_str(), body["message"].as_str()),
        (Some(code), Some(message))
            if code.eq_ignore_ascii_case("unauthenticated")
                && message.trim().eq_ignore_ascii_case("missing CSRF token")
    )
}

/// An error the user can do something about, as opposed to "that product is not
/// running". `post_rpc` only ever yields `Http`/`Transport`/`Other`, so the
/// authentication statuses are the whole set, apart from `agy`'s precise
/// missing-CSRF response.
fn is_actionable(e: &AppError) -> bool {
    matches!(e, AppError::Http { status, .. } if *status == 401 || *status == 403)
        && !is_missing_csrf(e)
}

/// A TLS listener answering the plaintext JSON-RPC probe.
///
/// Each Antigravity product binds two ports: JSON-RPC in the clear on one and
/// HTTPS on another. `probe_order` deliberately tries the RPC listener first
/// and leaves the TLS one behind it, so reaching the TLS port at all means the
/// RPC port already had its say. Go's `net/http.Server` answers cleartext on a
/// TLS listener with this fixed `400`, which describes our own probe rather
/// than anything wrong with the product — as a diagnosis it is noise that
/// always arrives last and therefore always wins.
///
/// Matched on the body, not the status alone: a real `400` from the language
/// server is a genuine complaint about the request and must keep outranking it.
fn is_tls_echo(e: &AppError) -> bool {
    matches!(
        e,
        AppError::Http { status: 400, body } if body.contains("HTTP request to an HTTPS server")
    )
}

async fn fetch_live(
    client: &reqwest::Client,
    session: Result<Session>,
) -> Result<AntigravitySnapshot> {
    let session = session?;
    let quota = post_rpc(client, &session.base, session.csrf.as_deref(), QUOTA_RPC).await?;
    let mut snap = parse_quota_summary(&quota, session.plan)?;
    snap.account = session.account;
    Ok(snap)
}

// ---------------------------------------------------------------------------
// Remote fallback — the saved Google session against the Cloud Code API
// ---------------------------------------------------------------------------

/// The saved Google session, or why there is none. `credential` is the test
/// seam for the keyring blob.
///
/// No saved session leaves the user exactly where the local probe left them,
/// plus the one thing they can now do about it.
fn saved_session(
    credential: SavedCredential<'_>,
    reason: RemoteFallbackReason,
) -> Result<StoredToken> {
    let raw = match credential {
        SavedCredential::Keyring => credential::read()?,
        SavedCredential::Blob(blob) => Some(blob.to_string()),
        SavedCredential::Absent => None,
    };
    let Some(raw) = raw else {
        return Err(AppError::Credentials(format!(
            "{} {NO_SAVED_SESSION}",
            reason.message()
        )));
    };
    credential::parse_keyring_blob(&raw)
}

/// Cache attribution for a remote snapshot. Deliberately not the local
/// fingerprint's format: the two are computed from different inputs and a
/// collision would let one source's cache stand in for the other's.
fn remote_account(fingerprint: &str) -> String {
    format!("acct:{fingerprint}")
}

fn session_expired() -> AppError {
    AppError::Credentials(SESSION_EXPIRED.into())
}

/// The saved session can only be renewed with Antigravity's OAuth client,
/// which this program does not ship (a secret-shaped literal in source trips
/// every secret scanner); the config names the two keys that provide it.
fn refresh_unconfigured() -> AppError {
    AppError::Credentials(
        "Antigravity's saved Google session expired and ai-usagebar has no OAuth client to \
         refresh it; open Antigravity to sign in again, or set [antigravity] oauth_client_id \
         and oauth_client_secret in config.toml"
            .into(),
    )
}

fn is_auth_rejection(e: &AppError) -> bool {
    matches!(
        e,
        AppError::Http {
            status: 401 | 403,
            ..
        }
    )
}

/// An access token to present, and whether it was minted just now — a `401`
/// against a token this fresh is the session itself being gone, not a stale
/// token worth refreshing again.
struct AccessToken {
    value: String,
    just_refreshed: bool,
}

/// Pick the freshest usable access token without going to the network: the
/// one this program persisted after its last refresh, if it outlives the
/// keyring's and is not about to expire; else the keyring's own while it
/// lasts. Only when both are spent does this refresh.
async fn resolve_access_token(
    client: &reqwest::Client,
    oauth: Option<&cloud::OauthClient>,
    endpoints: &cloud::Endpoints,
    oauth_path: &std::path::Path,
    token: &StoredToken,
    now: DateTime<Utc>,
) -> Result<AccessToken> {
    if let Some(persisted) = cloud::read_persisted(oauth_path, &token.fingerprint)
        && token
            .expires_at
            .is_none_or(|keyring| persisted.expires_at > keyring)
        && !cloud::needs_refresh(Some(persisted.expires_at), now)
    {
        return Ok(AccessToken {
            value: persisted.access_token,
            just_refreshed: false,
        });
    }
    if !cloud::needs_refresh(token.expires_at, now) {
        return Ok(AccessToken {
            value: token.access_token.clone(),
            just_refreshed: false,
        });
    }
    refresh_and_persist(client, oauth, endpoints, oauth_path, token).await
}

/// Mint a new access token off the saved refresh token and remember it, so
/// the next poll does not spend another round-trip on the same refresh. A
/// session with no refresh token cannot be renewed here at all.
async fn refresh_and_persist(
    client: &reqwest::Client,
    oauth: Option<&cloud::OauthClient>,
    endpoints: &cloud::Endpoints,
    oauth_path: &std::path::Path,
    token: &StoredToken,
) -> Result<AccessToken> {
    let Some(refresh_token) = token.refresh_token.as_deref() else {
        return Err(session_expired());
    };
    let Some(oauth) = oauth else {
        return Err(refresh_unconfigured());
    };
    let refreshed = cloud::refresh(client, &endpoints.token, oauth, refresh_token).await?;
    cloud::write_persisted(
        oauth_path,
        &cloud::PersistedOAuth {
            fingerprint: token.fingerprint.clone(),
            access_token: refreshed.access_token.clone(),
            expires_at: refreshed.expires_at,
        },
    )?;
    Ok(AccessToken {
        value: refreshed.access_token,
        just_refreshed: true,
    })
}

/// Quota through the Cloud Code API, as the signed-in Google account.
///
/// A rejected token gets one refresh and one retry, unless it was refreshed a
/// moment ago — then the rejection is Google's verdict on the session, and
/// the user has to sign in again. The plan name is best-effort: the summary is
/// the figure, and a missing name must not cost it.
async fn fetch_remote(
    client: &reqwest::Client,
    cache: &Cache,
    oauth: Option<&cloud::OauthClient>,
    endpoints: &cloud::Endpoints,
    token: Result<StoredToken>,
    now: DateTime<Utc>,
) -> Result<AntigravitySnapshot> {
    let token = token?;
    let oauth_path = cloud::oauth_cache_path(cache);
    let mut access =
        resolve_access_token(client, oauth, endpoints, &oauth_path, &token, now).await?;

    let quota = match cloud::fetch_quota(client, endpoints, &access.value).await {
        Err(e) if is_auth_rejection(&e) && !access.just_refreshed => {
            access = refresh_and_persist(client, oauth, endpoints, &oauth_path, &token).await?;
            cloud::fetch_quota(client, endpoints, &access.value)
                .await
                .map_err(|e| {
                    if is_auth_rejection(&e) {
                        session_expired()
                    } else {
                        e
                    }
                })?
        }
        Err(e) if is_auth_rejection(&e) => return Err(session_expired()),
        other => other?,
    };

    let plan = cloud::fetch_plan(client, endpoints, &access.value)
        .await
        .unwrap_or_else(|| DEFAULT_PLAN.to_string());
    let mut snap = parse_quota_summary(&quota, plan)?;
    snap.account = remote_account(&token.fingerprint);
    snap.source = AntigravitySource::Remote;
    Ok(snap)
}

/// Identity of the signed-in account, fingerprinted rather than stored in
/// clear — the cache only needs a change detector, not the address itself.
/// An unidentifiable response yields a stable "unknown" bucket so two such
/// responses still compare equal.
fn account_key(user_status: &serde_json::Value) -> String {
    let email = user_status["userStatus"]["email"]
        .as_str()
        .filter(|s| !s.is_empty());
    match email {
        Some(e) => {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            e.hash(&mut h);
            format!("acct:{:016x}", h.finish())
        }
        None => "acct:unknown".to_string(),
    }
}

/// The desktop products embed a CSRF token in the HTML served at `/`. The
/// `agy` CLI serves no such page, so a missing token is recorded as `None` and
/// its precise rejection is classified after the RPC probe.
async fn fetch_csrf(client: &reqwest::Client, base: &str) -> Option<String> {
    let resp = client.get(base).timeout(HTTP_TIMEOUT).send().await.ok()?;
    // Bounded like every other response this crate reads: a local server is
    // still an untrusted source of unbounded bytes.
    let bytes = crate::vendor::read_body_capped(resp, crate::vendor::MAX_BODY_BYTES)
        .await
        .ok()?;
    let html = String::from_utf8_lossy(&bytes);
    html.split("csrfToken\":\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
}

async fn post_rpc(
    client: &reqwest::Client,
    base: &str,
    csrf: Option<&str>,
    rpc: &str,
) -> Result<serde_json::Value> {
    let mut req = client
        .post(format!("{base}/{rpc}"))
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(HTTP_TIMEOUT);
    if let Some(token) = csrf {
        req = req.header("x-codeium-csrf-token", token);
    }
    let resp = req.send().await?;

    let status = resp.status();
    // Cap error bodies too. A local endpoint is still untrusted, and reading a
    // non-2xx response with `text()` would bypass the invariant enforced for
    // successful JSON responses.
    let bytes = crate::vendor::read_body_capped(resp, crate::vendor::MAX_BODY_BYTES).await?;
    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes).into_owned();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }

    Ok(serde_json::from_slice(&bytes)?)
}

pub fn plan_from_status(v: &serde_json::Value) -> String {
    v["userStatus"]["userTier"]["name"]
        .as_str()
        .or_else(|| v["userStatus"]["userTier"]["description"].as_str())
        .or_else(|| v["userStatus"]["planStatus"]["planInfo"]["planName"].as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_PLAN)
        .to_string()
}

// ---------------------------------------------------------------------------
// Quota parsing
// ---------------------------------------------------------------------------

/// Map a `RetrieveUserQuotaSummary` payload onto the four usage windows.
///
/// Buckets are keyed by `bucketId` (`gemini-5h`, `gemini-weekly`, `3p-5h`,
/// `3p-weekly`), falling back to the group display name plus the `window`
/// discriminator so a renamed bucket id still lands in the right slot.
/// One bucket, named the way the response named it.
fn describe_bucket(group_name: &str, id: &str, window: Option<&str>) -> String {
    let id = if id.is_empty() { "<unnamed>" } else { id };
    let group = if group_name.is_empty() {
        String::new()
    } else {
        format!(" in {group_name:?}")
    };
    match window {
        Some(window) if !window.is_empty() => format!("{id} (window {window}){group}"),
        _ => format!("{id}{group}"),
    }
}

/// A quota summary with nothing we can render. Naming the buckets that *were*
/// present turns a report of this into something actionable — the alternative
/// says only what we wanted, which tells neither the user nor a maintainer
/// whether the plan has no such pool, the product renamed one, or a new
/// cadence appeared.
fn no_usable_bucket(seen: &[String]) -> AppError {
    if seen.is_empty() {
        return AppError::Other(
            "antigravity: quota summary has no buckets at all — the running product may \
             not have a quota for this account yet"
                .into(),
        );
    }
    AppError::Other(format!(
        "antigravity: quota summary has no bucket in a window we recognise (5h or \
         weekly, Gemini or Claude/GPT); it offered: {}",
        seen.join(", ")
    ))
}

pub fn parse_quota_summary(v: &serde_json::Value, plan: String) -> Result<AntigravitySnapshot> {
    let groups = v["response"]["groups"]
        .as_array()
        .or_else(|| v["groups"].as_array())
        .ok_or_else(|| AppError::Other("antigravity: quota summary has no groups".into()))?;

    let mut gemini_5h = None;
    let mut gemini_weekly = None;
    let mut tp_5h = None;
    let mut tp_weekly = None;
    // What the response actually offered, so a summary we cannot use says so
    // instead of only naming what it wanted. Bucket ids and group names are
    // quota vocabulary, not account data.
    let mut seen: Vec<String> = Vec::new();

    for group in groups {
        let group_name = group["displayName"].as_str().unwrap_or_default();
        let Some(buckets) = group["buckets"].as_array() else {
            continue;
        };
        for bucket in buckets {
            let id = bucket["bucketId"].as_str().unwrap_or_default();
            seen.push(describe_bucket(group_name, id, bucket["window"].as_str()));
            let window = bucket["window"].as_str().unwrap_or_default();
            let is_weekly = if id.ends_with("weekly") || window == "weekly" {
                true
            } else if id.ends_with("5h") || window == "5h" {
                false
            } else {
                // A new cadence is not a 5-hour bucket by default. Ignore it
                // so it cannot overwrite a known slot.
                continue;
            };
            let is_gemini = if id.starts_with("gemini") {
                true
            } else if id.starts_with("3p") {
                false
            } else if group_name.contains("Gemini") {
                true
            } else if group_name.contains("Claude") || group_name.contains("GPT") {
                false
            } else {
                // Likewise, an unrelated future group is not implicitly the
                // third-party pool.
                continue;
            };

            let (slot, slot_name) = match (is_gemini, is_weekly) {
                (true, false) => (&mut gemini_5h, "Gemini 5h"),
                (true, true) => (&mut gemini_weekly, "Gemini weekly"),
                (false, false) => (&mut tp_5h, "Claude/GPT 5h"),
                (false, true) => (&mut tp_weekly, "Claude/GPT weekly"),
            };
            let parsed = usage_window(bucket, is_weekly)?;
            if slot.replace(parsed).is_some() {
                return Err(AppError::Schema(format!(
                    "antigravity: duplicate {slot_name} bucket"
                )));
            }
        }
    }

    // Not every product offers every window: Antigravity CLI 1.1.22 returns
    // weekly buckets only. One recognised window is enough to render — what
    // must never happen is showing a figure for a window that did not arrive.
    if gemini_5h.is_none() && gemini_weekly.is_none() && tp_5h.is_none() && tp_weekly.is_none() {
        return Err(no_usable_bucket(&seen));
    }

    Ok(AntigravitySnapshot {
        plan,
        // Stamped by the caller, which is what knows the session's identity
        // and which path it came through.
        account: String::new(),
        source: AntigravitySource::Local,
        session: gemini_5h,
        weekly: gemini_weekly,
        third_party_session: tp_5h,
        third_party_weekly: tp_weekly,
    })
}

/// `remainingFraction` is required and must be finite: defaulting a missing or
/// drifted value to 1.0 would report a reassuring "0% used" for a window whose
/// real state is unknown, and cache it.
fn usage_window(bucket: &serde_json::Value, is_weekly: bool) -> Result<UsageWindow> {
    let remaining = bucket["remainingFraction"]
        .as_f64()
        .filter(|f| f.is_finite() && (0.0..=1.0).contains(f))
        .ok_or_else(|| {
            AppError::Schema(format!(
                "antigravity: bucket {} has no valid remainingFraction in 0..=1",
                bucket["bucketId"].as_str().unwrap_or("<unnamed>")
            ))
        })?;
    Ok(UsageWindow {
        utilization_pct: pct_used(remaining),
        resets_at: parse_reset(&bucket["resetTime"], "quota resetTime")?,
        window_duration: if is_weekly {
            chrono::Duration::days(7)
        } else {
            chrono::Duration::hours(5)
        },
    })
}

/// The API reports how much is *left*; every other vendor here reports how much
/// is *spent*.
fn pct_used(remaining_fraction: f64) -> i32 {
    let used = (1.0 - remaining_fraction) * 100.0;
    used.round() as i32
}

fn parse_reset(value: &serde_json::Value, field: &str) -> Result<Option<DateTime<Utc>>> {
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => DateTime::parse_from_rfc3339(s)
            .map(|dt| Some(dt.with_timezone(&Utc)))
            .map_err(|_| AppError::Schema(format!("antigravity: invalid {field}"))),
        _ => Err(AppError::Schema(format!(
            "antigravity: {field} must be a timestamp or null"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Language server discovery
// ---------------------------------------------------------------------------

/// Base URLs worth probing, most specific first.
fn candidate_bases() -> Vec<String> {
    candidate_bases_with(
        std::env::var("ANTIGRAVITY_LS_ADDRESS").ok().as_deref(),
        discover_ls_ports(),
    )
}

/// Test seam for [`candidate_bases`] — takes the address override and the
/// discovered ports instead of reading the environment and `/proc`.
fn candidate_bases_with(override_addr: Option<&str>, discovered: Vec<u16>) -> Vec<String> {
    let mut bases = Vec::new();
    if let Some(base) = override_addr.and_then(normalize_base) {
        bases.push(base);
    }

    // No hardcoded fallback port on purpose: the server always binds with
    // `--https_server_port 0`, so its port is drawn from the ephemeral range
    // and cannot be guessed. Probing a fixed one would just poke whatever
    // unrelated process happens to own it. Discovered ports follow any
    // explicit override as fallback, with duplicates omitted.
    for p in discovered {
        let candidate = format!("http://127.0.0.1:{p}");
        if !bases.contains(&candidate) {
            bases.push(candidate);
        }
    }

    bases
}

/// Turn a configured address into a base URL: trim surrounding whitespace,
/// supply the default scheme when it is missing, and drop trailing slashes so
/// the RPC paths built on top do not come out with a double slash.
///
/// Returns `None` when nothing but a scheme survives. `ANTIGRAVITY_LS_ADDRESS`
/// is user input, and a value like `"/"` carries no authority to connect to;
/// admitting it as a candidate would spend a probe to learn what is already
/// knowable here.
fn normalize_base(addr: &str) -> Option<String> {
    let trimmed = addr.trim();
    let (scheme, authority) = match trimmed.split_once("://") {
        Some((scheme @ ("http" | "https"), rest)) => (scheme, rest),
        _ => ("http", trimmed),
    };
    let authority = authority.trim_end_matches('/');
    (!authority.is_empty()).then(|| format!("{scheme}://{authority}"))
}

/// Does this process look like one of the three Antigravity products?
///
/// Antigravity 2.0 and the IDE spawn a separate `language_server` child, while
/// the `agy` CLI embeds the same CSRF/RPC surface in its own process — so
/// matching on the server binary alone would miss a CLI-only install. `comm` is
/// truncated to 15 bytes by the kernel, which `language_server` exactly fills.
fn is_antigravity_process(comm: &str, exe: Option<&str>) -> bool {
    let comm = comm.trim().to_lowercase();
    let comm = comm.strip_suffix(".exe").unwrap_or(&comm);
    if comm.contains("language_server") || comm == "agy" || comm == "antigravity" {
        return true;
    }
    exe.is_some_and(|p| {
        let p = p.to_lowercase().replace('\\', "/");
        let p = p.strip_suffix(".exe").unwrap_or(&p);
        p.contains("antigravity") || p.ends_with("/agy")
    })
}

/// Flatten per-process listener ports into the order they should be probed.
///
/// Each Antigravity product binds an HTTPS/TLS listener and the unencrypted
/// HTTP JSON-RPC listener that serves `GetUserStatus` and
/// `RetrieveUserQuotaSummary`. Both are ephemeral, but they are bound in that
/// order, so in practice the RPC listener draws the higher number and probing
/// high-to-low reaches it first — which keeps Go's `net/http.Server` from
/// logging a TLS handshake error for every unencrypted request that lands on
/// its HTTPS listener.
///
/// Sorting every discovered port as one descending set only gets that right
/// for a single process, because the high/low tendency holds *within* a
/// product and says nothing across two of them: with Antigravity 2.0 and an
/// `agy` session both up, one product's TLS port can sort above the other's
/// RPC port. Ports are therefore grouped per pid, sorted high-to-low inside
/// each group, and taken rank by rank — every product's highest port, then
/// every product's second-highest, and so on. Where each product shows both
/// listeners that puts every RPC one ahead of every TLS one.
///
/// This stays a preference, not a guarantee, and the two-listener shape is the
/// assumption it rests on: a product caught mid-startup, with only its TLS port
/// bound so far, sits alone at rank 0 and is probed first. Every candidate is
/// probed regardless, so a mis-ranked one costs an extra round-trip and a line
/// on `agy`'s stderr, nothing more.
///
/// Order among products is arbitrary — all of them report the same
/// account-wide quota, so whichever answers first is authoritative — and pid
/// order is used only to keep the result reproducible, since `/proc`, `lsof`
/// and the Windows TCP table each enumerate in their own order.
#[cfg(any(test, target_os = "linux", target_os = "macos", target_os = "windows"))]
fn probe_order(per_pid: std::collections::BTreeMap<u32, Vec<u16>>) -> Vec<u16> {
    let groups: Vec<Vec<u16>> = per_pid
        .into_values()
        .map(|mut group| {
            group.sort_unstable_by(|a, b| b.cmp(a));
            // Within a product a port is one listener however many rows named
            // it — a dual-stack bind reports the same port from both
            // `/proc/net/tcp` and `tcp6`. Collapsing them here keeps a rank
            // meaning "the Nth listener" rather than "the Nth row".
            group.dedup();
            group
        })
        .collect();

    let mut ports: Vec<u16> = Vec::new();
    for rank in 0..groups.iter().map(Vec::len).max().unwrap_or(0) {
        for port in groups.iter().filter_map(|group| group.get(rank)) {
            if !ports.contains(port) {
                ports.push(*port);
            }
        }
    }
    ports
}

/// Loopback ports listened on by any running Antigravity product.
///
/// Reads `/proc` directly rather than shelling out to `ss`/`lsof`: find the
/// candidate pids, collect their socket inodes, then keep the listening TCP
/// entries owning one of those inodes. All three products report the *same*
/// shared quota, so whichever answers first is authoritative.
#[cfg(target_os = "linux")]
pub(crate) fn discover_ls_ports() -> Vec<u16> {
    use std::collections::{BTreeMap, HashMap};

    // Socket inode -> owning pid, so the ports found in `/proc/net` can be
    // grouped back per process for `probe_order`.
    let mut owners: HashMap<u64, u32> = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let pid_dir = entry.path();
        let Some(pid) = pid_dir
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(comm) = std::fs::read_to_string(pid_dir.join("comm")) else {
            continue;
        };
        let exe = std::fs::read_link(pid_dir.join("exe")).ok();
        if !is_antigravity_process(&comm, exe.as_deref().and_then(|p| p.to_str())) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(pid_dir.join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            if let Some(ino) = target
                .to_str()
                .and_then(|s| s.strip_prefix("socket:["))
                .and_then(|s| s.strip_suffix(']'))
                .and_then(|s| s.parse::<u64>().ok())
            {
                owners.insert(ino, pid);
            }
        }
    }

    if owners.is_empty() {
        return Vec::new();
    }

    let mut per_pid: BTreeMap<u32, Vec<u16>> = BTreeMap::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(contents) = std::fs::read_to_string(table) else {
            continue;
        };
        for line in contents.lines().skip(1) {
            if let Some((port, ino)) = parse_proc_net_line(line)
                && let Some(&pid) = owners.get(&ino)
            {
                per_pid.entry(pid).or_default().push(port);
            }
        }
    }
    probe_order(per_pid)
}

/// macOS has no `/proc`, so fall back to `lsof` (present on every macOS
/// install by default, unlike Linux where shelling out was deliberately
/// avoided — see the doc comment above). `-F pcn` asks for machine-parsable
/// output: one `p<pid>` line per process, one `c<command>` line for its name,
/// then an `n<address>` line per matching socket already filtered down to
/// listening TCP sockets by `-iTCP -sTCP:LISTEN`.
#[cfg(target_os = "macos")]
pub(crate) fn discover_ls_ports() -> Vec<u16> {
    let Ok(output) = std::process::Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pcn"])
        .output()
    else {
        return Vec::new();
    };
    // A non-zero exit still emits usable output for the fds it *could* read,
    // so parse regardless of status — an empty/garbled stdout just parses to
    // an empty port list.
    parse_lsof_pcn(&String::from_utf8_lossy(&output.stdout))
}

/// Pure parser for `lsof -F pcn` output, kept separate from process spawning
/// so the parsing logic is unit-testable without shelling out. Compiled under
/// `test` on every platform, like [`matching_windows_ports`], so its tests are
/// not macOS-only.
#[cfg(any(test, target_os = "macos"))]
fn parse_lsof_pcn(output: &str) -> Vec<u16> {
    let mut per_pid: std::collections::BTreeMap<u32, Vec<u16>> = std::collections::BTreeMap::new();
    // The pid arrives on the `p` line and the command name on the `c` line
    // right after it, so hold the pid until the name confirms it is ours.
    let mut pid = None;
    let mut owner = None;
    for line in output.lines() {
        let Some(rest) = line.get(1..) else { continue };
        match line.as_bytes().first() {
            Some(b'p') => {
                pid = rest.parse::<u32>().ok();
                owner = None;
            }
            Some(b'c') => owner = pid.filter(|_| is_antigravity_process(rest, None)),
            Some(b'n') => {
                if let Some(pid) = owner
                    && let Some(port) = rest.rsplit(':').next().and_then(|p| p.parse::<u16>().ok())
                {
                    per_pid.entry(pid).or_default().push(port);
                }
            }
            _ => {}
        }
    }
    probe_order(per_pid)
}

#[cfg(any(test, target_os = "windows"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowsTcpRow {
    local_addr: [u8; 4],
    local_port: u32,
    pid: u32,
}

#[cfg(any(test, target_os = "windows"))]
fn decode_windows_process_name(raw: &[u16]) -> String {
    let end = raw.iter().position(|&unit| unit == 0).unwrap_or(raw.len());
    String::from_utf16_lossy(&raw[..end])
}

#[cfg(any(test, target_os = "windows"))]
fn matching_windows_process_ids(processes: &[(u32, String)]) -> std::collections::HashSet<u32> {
    processes
        .iter()
        .filter(|(_, name)| is_antigravity_process(name, None))
        .map(|(pid, _)| *pid)
        .collect()
}

/// Loopback ports owned by the matching processes, grouped per pid and handed
/// to [`probe_order`], which explains why the grouping matters.
#[cfg(any(test, target_os = "windows"))]
fn matching_windows_ports(
    pids: &std::collections::HashSet<u32>,
    rows: &[WindowsTcpRow],
) -> Vec<u16> {
    let mut per_pid: std::collections::BTreeMap<u32, Vec<u16>> = std::collections::BTreeMap::new();
    for row in rows {
        if !pids.contains(&row.pid) || row.local_addr != [127, 0, 0, 1] {
            continue;
        }
        let port = u16::from_be((row.local_port & u32::from(u16::MAX)) as u16);
        if port != 0 {
            per_pid.entry(row.pid).or_default().push(port);
        }
    }
    probe_order(per_pid)
}

#[cfg(any(test, target_os = "windows"))]
fn checked_windows_row_count(
    buffer_len: usize,
    rows_offset: usize,
    row_size: usize,
    declared: usize,
) -> Option<usize> {
    let rows_len = row_size.checked_mul(declared)?;
    let end = rows_offset.checked_add(rows_len)?;
    (row_size != 0 && end <= buffer_len).then_some(declared)
}

#[cfg(target_os = "windows")]
struct WindowsHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(target_os = "windows")]
impl Drop for WindowsHandle {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_processes() -> Vec<(u32, String)> {
    use std::mem::size_of;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    let handle = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if handle == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let snapshot = WindowsHandle(handle);
    let mut entry = PROCESSENTRY32W::default();
    let Ok(entry_size) = u32::try_from(size_of::<PROCESSENTRY32W>()) else {
        return Vec::new();
    };
    entry.dwSize = entry_size;
    if unsafe { Process32FirstW(snapshot.0, &mut entry) } == 0 {
        return Vec::new();
    }

    let mut processes = Vec::new();
    loop {
        processes.push((
            entry.th32ProcessID,
            decode_windows_process_name(&entry.szExeFile),
        ));
        if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
            break;
        }
    }
    processes
}

#[cfg(target_os = "windows")]
fn parse_windows_tcp_rows(buffer: &[u32], used_bytes: usize) -> Vec<WindowsTcpRow> {
    use std::mem::{offset_of, size_of, size_of_val};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
    };

    let available = used_bytes.min(size_of_val(buffer));
    let rows_offset = offset_of!(MIB_TCPTABLE_OWNER_PID, table);
    if available < size_of::<u32>() || available < rows_offset {
        return Vec::new();
    }
    let base = buffer.as_ptr().cast::<u8>();
    let declared = unsafe { base.cast::<u32>().read_unaligned() } as usize;
    if checked_windows_row_count(
        available,
        rows_offset,
        size_of::<MIB_TCPROW_OWNER_PID>(),
        declared,
    )
    .is_none()
    {
        return Vec::new();
    }

    let rows = unsafe { base.add(rows_offset).cast::<MIB_TCPROW_OWNER_PID>() };
    (0..declared)
        .map(|index| unsafe { rows.add(index).read_unaligned() })
        .map(|row| WindowsTcpRow {
            local_addr: row.dwLocalAddr.to_ne_bytes(),
            local_port: row.dwLocalPort,
            pid: row.dwOwningPid,
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn windows_tcp_rows() -> Vec<WindowsTcpRow> {
    use std::mem::size_of;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, TCP_TABLE_OWNER_PID_LISTENER,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET;

    let mut size = 0u32;
    let status = unsafe {
        GetExtendedTcpTable(
            null_mut(),
            &mut size,
            0,
            u32::from(AF_INET),
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    if status != ERROR_INSUFFICIENT_BUFFER {
        return Vec::new();
    }

    for _ in 0..3 {
        let Some(words) = (size as usize)
            .checked_add(size_of::<u32>() - 1)
            .map(|bytes| bytes / size_of::<u32>())
        else {
            return Vec::new();
        };
        if words == 0 {
            return Vec::new();
        }
        let mut buffer = Vec::<u32>::new();
        if buffer.try_reserve_exact(words).is_err() {
            return Vec::new();
        }
        buffer.resize(words, 0);
        let mut used = size;
        let status = unsafe {
            GetExtendedTcpTable(
                buffer.as_mut_ptr().cast(),
                &mut used,
                0,
                u32::from(AF_INET),
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if status == ERROR_INSUFFICIENT_BUFFER {
            size = used;
            continue;
        }
        if status != 0 {
            return Vec::new();
        }
        return parse_windows_tcp_rows(&buffer, used as usize);
    }
    Vec::new()
}

#[cfg(target_os = "windows")]
pub(crate) fn discover_ls_ports() -> Vec<u16> {
    let pids = matching_windows_process_ids(&windows_processes());
    if pids.is_empty() {
        return Vec::new();
    }
    matching_windows_ports(&pids, &windows_tcp_rows())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(crate) fn discover_ls_ports() -> Vec<u16> {
    Vec::new()
}

/// Pull `(local_port, inode)` out of a listening row of `/proc/net/tcp`.
/// Columns: `sl local_address rem_address st ... uid timeout inode`.
#[cfg(target_os = "linux")]
fn parse_proc_net_line(line: &str) -> Option<(u16, u64)> {
    let cols: Vec<&str> = line.split_whitespace().collect();
    if cols.len() < 10 {
        return None;
    }
    // 0x0A == TCP_LISTEN. Anything else is an established/closing socket.
    if cols[3] != "0A" {
        return None;
    }
    let port = u16::from_str_radix(cols[1].split(':').nth(1)?, 16).ok()?;
    let inode = cols[9].parse::<u64>().ok()?;
    Some((port, inode))
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

fn fallback_silent(cache: &Cache, now: DateTime<Utc>, original: AppError) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, None, original, |bytes| {
        parse_cache_at(bytes, None, now)
    })
}

/// Serve the stale cache when there is one. With no cache to fall back on,
/// surface `reason` — the actual diagnosis, e.g. "no local language server
/// found" — rather than a generic cache-miss that tells the user nothing about
/// what to do. This is the first-run path: no cache yet and Antigravity closed.
fn fallback_with_error(
    cache: &Cache,
    last_error: Option<(u16, String)>,
    reason: AppError,
    now: DateTime<Utc>,
) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, last_error, reason, |bytes| {
        parse_cache_at(bytes, None, now)
    })
}

fn reuse_cache(
    bytes: Vec<u8>,
    cache: &Cache,
    stale: bool,
    account: Option<&str>,
    now: DateTime<Utc>,
) -> Result<FetchOutcome> {
    let snap = parse_cache_at(&bytes, account, now)?;
    Ok(crate::outcome::Outcome::cached(snap, cache, stale))
}

/// `account` is the fingerprint of the currently signed-in account, or `None`
/// when no local server answered. A payload belonging to a different account is
/// rejected so a Google-account switch cannot show the previous account's
/// quota. With `None` we cannot verify — but nothing is consuming quota while
/// Antigravity is down, so the last known figures are the best available truth.
pub fn parse_cache(bytes: &[u8], account: Option<&str>) -> Result<AntigravitySnapshot> {
    parse_cache_at(bytes, account, Utc::now())
}

/// A cached window whose reset has already passed describes a period that has
/// since rolled over: the real figure is back near zero while the payload still
/// carries the old one, and its countdown is pinned at "now". Serving that is
/// presenting a known-obsolete number as current, so the payload is refused and
/// the caller reports that Antigravity needs to be running.
///
/// This matters more here than for other vendors: "nothing running" is the
/// normal state for Antigravity, and `MAX_STALE` is seven days — far past the
/// five hours after which the session window is guaranteed wrong.
fn expired_window(snap: &AntigravitySnapshot, now: DateTime<Utc>) -> Option<&'static str> {
    [
        ("Gemini 5h", snap.session.as_ref()),
        ("Gemini weekly", snap.weekly.as_ref()),
        ("Claude & GPT OSS 5h", snap.third_party_session.as_ref()),
        ("Claude & GPT OSS weekly", snap.third_party_weekly.as_ref()),
    ]
    .into_iter()
    .find(|(_, w)| w.and_then(|w| w.resets_at).is_some_and(|r| r <= now))
    .map(|(name, _)| name)
}

pub fn parse_cache_at(
    bytes: &[u8],
    account: Option<&str>,
    now: DateTime<Utc>,
) -> Result<AntigravitySnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;

    let cached_account = v.get("account").and_then(serde_json::Value::as_str);
    if let Some(expected) = account
        && cached_account != Some(expected)
    {
        return Err(AppError::Schema(
            "antigravity cache belongs to a different account; refetching".into(),
        ));
    }

    // The Gemini windows are required. Defaulting a missing or truncated field
    // to 0 would render a confident "0% used" and keep serving it for the rest
    // of the TTL; returning an error makes the caller fall through to a live
    // fetch instead of displaying a fabricated snapshot.
    // An absent window and a truncated payload look alike unless we insist on
    // the difference: `snap_to_json` always writes every key, so an explicit
    // `null` means "this product reported no such window" while a *missing*
    // key means the document is not one we wrote whole. Only the first is a
    // snapshot; the second must refetch rather than render a window short.
    let cached_pct = |pct_key: &'static str| -> Result<Option<i32>> {
        match v.get(pct_key) {
            None => Err(AppError::Schema(format!(
                "antigravity: cached payload is missing {pct_key}"
            ))),
            Some(serde_json::Value::Null) => Ok(None),
            Some(value) => value
                .as_i64()
                .filter(|pct| (0..=100).contains(pct))
                .map(|pct| Some(pct as i32))
                .ok_or_else(|| {
                    AppError::Schema(format!(
                        "antigravity: cached {pct_key} must be an integer in 0..=100"
                    ))
                }),
        }
    };

    let optional = |pct_key: &'static str, reset_key: &str, weekly: bool| {
        let Some(pct) = cached_pct(pct_key)? else {
            return Ok(None);
        };
        Ok::<_, AppError>(Some(UsageWindow {
            utilization_pct: pct,
            resets_at: parse_reset(&v[reset_key], reset_key)?,
            window_duration: if weekly {
                chrono::Duration::days(7)
            } else {
                chrono::Duration::hours(5)
            },
        }))
    };

    let snap = AntigravitySnapshot {
        plan: v["plan"].as_str().unwrap_or(DEFAULT_PLAN).to_string(),
        account: cached_account.unwrap_or_default().to_string(),
        // Payloads written before the remote path existed were all local.
        source: v["source"]
            .as_str()
            .and_then(AntigravitySource::parse)
            .unwrap_or_default(),
        session: optional("session_pct", "session_reset", false)?,
        weekly: optional("weekly_pct", "weekly_reset", true)?,
        third_party_session: optional("tp_session_pct", "tp_session_reset", false)?,
        third_party_weekly: optional("tp_weekly_pct", "tp_weekly_reset", true)?,
    };

    // A cache with no window left is not a snapshot; refetch rather than draw
    // an empty panel from it. Mirrors the live parse.
    if snap.session.is_none()
        && snap.weekly.is_none()
        && snap.third_party_session.is_none()
        && snap.third_party_weekly.is_none()
    {
        return Err(AppError::Schema(
            "antigravity cache holds no usable window; refetching".into(),
        ));
    }

    if let Some(window) = expired_window(&snap, now) {
        return Err(AppError::Schema(format!(
            "antigravity cache is past its {window} reset; refetching"
        )));
    }
    Ok(snap)
}

pub fn snap_to_json(snap: &AntigravitySnapshot) -> serde_json::Value {
    serde_json::json!({
        "plan": snap.plan,
        "account": snap.account,
        "source": snap.source.as_str(),
        "session_pct": snap.session.as_ref().map(|w| w.utilization_pct),
        "session_reset": snap.session.as_ref().and_then(|w| w.resets_at.map(|dt| dt.to_rfc3339())),
        "weekly_pct": snap.weekly.as_ref().map(|w| w.utilization_pct),
        "weekly_reset": snap.weekly.as_ref().and_then(|w| w.resets_at.map(|dt| dt.to_rfc3339())),
        "tp_session_pct": snap.third_party_session.as_ref().map(|w| w.utilization_pct),
        "tp_session_reset": snap.third_party_session.as_ref().and_then(|w| w.resets_at.map(|dt| dt.to_rfc3339())),
        "tp_weekly_pct": snap.third_party_weekly.as_ref().map(|w| w.utilization_pct),
        "tp_weekly_reset": snap.third_party_weekly.as_ref().and_then(|w| w.resets_at.map(|dt| dt.to_rfc3339())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from a real `RetrieveUserQuotaSummary` response on 2026-07-22
    /// (Antigravity 2.0 build 2.3.1, `agy` 1.1.5), then trimmed. Percentages
    /// were edited to distinct non-zero values so a slot mix-up cannot pass.
    const QUOTA_JSON: &str = r#"{
      "response": {
        "groups": [
          {
            "displayName": "Gemini Models",
            "buckets": [
              {"bucketId": "gemini-weekly", "displayName": "Weekly Limit",
               "window": "weekly", "remainingFraction": 0.9191212,
               "resetTime": "2026-07-28T17:39:58Z"},
              {"bucketId": "gemini-5h", "displayName": "Five Hour Limit",
               "window": "5h", "remainingFraction": 0.5672253,
               "resetTime": "2026-07-22T17:47:00Z"}
            ]
          },
          {
            "displayName": "Claude and GPT models",
            "buckets": [
              {"bucketId": "3p-weekly", "window": "weekly",
               "remainingFraction": 1, "resetTime": "2026-07-29T12:47:00Z"},
              {"bucketId": "3p-5h", "window": "5h",
               "remainingFraction": 0.25, "resetTime": "2026-07-22T17:47:00Z"}
            ]
          }
        ]
      }
    }"#;

    /// Fixed instant, earlier than every reset in the fixture. Using the wall
    /// clock here would make the suite start failing once those resets pass.
    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-22T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn parsed() -> AntigravitySnapshot {
        let v: serde_json::Value = serde_json::from_str(QUOTA_JSON).unwrap();
        parse_quota_summary(&v, "Google AI Pro".into()).unwrap()
    }

    #[test]
    fn quota_summary_maps_four_distinct_windows() {
        let snap = parsed();
        assert_eq!(snap.plan, "Google AI Pro");
        // remainingFraction is inverted into "used".
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 43);
        assert_eq!(snap.weekly.as_ref().unwrap().utilization_pct, 8);
        assert_eq!(
            snap.third_party_session.as_ref().unwrap().utilization_pct,
            75
        );
        assert_eq!(snap.third_party_weekly.as_ref().unwrap().utilization_pct, 0);
    }

    #[test]
    fn each_window_keeps_its_own_reset_time() {
        let snap = parsed();
        let at = |s: &str| Some(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc));
        assert_eq!(
            snap.session.as_ref().unwrap().resets_at,
            at("2026-07-22T17:47:00Z")
        );
        assert_eq!(
            snap.weekly.as_ref().unwrap().resets_at,
            at("2026-07-28T17:39:58Z")
        );
        assert_eq!(
            snap.third_party_weekly.as_ref().unwrap().resets_at,
            at("2026-07-29T12:47:00Z")
        );
        // Regression: weekly must never be a copy of the 5h window.
        assert_ne!(
            snap.session.as_ref().unwrap().resets_at,
            snap.weekly.as_ref().unwrap().resets_at
        );
    }

    #[test]
    fn window_durations_match_their_bucket() {
        let snap = parsed();
        assert_eq!(
            snap.session.as_ref().unwrap().window_duration,
            chrono::Duration::hours(5)
        );
        assert_eq!(
            snap.weekly.as_ref().unwrap().window_duration,
            chrono::Duration::days(7)
        );
        assert_eq!(
            snap.third_party_weekly.as_ref().unwrap().window_duration,
            chrono::Duration::days(7)
        );
    }

    #[test]
    fn groups_are_matched_by_display_name_when_bucket_ids_change() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"response":{"groups":[
              {"displayName":"Gemini Models","buckets":[
                {"bucketId":"x1","window":"5h","remainingFraction":0.5,"resetTime":"2026-07-22T17:47:00Z"},
                {"bucketId":"x2","window":"weekly","remainingFraction":0.9,"resetTime":"2026-07-28T17:39:58Z"}]},
              {"displayName":"Claude and GPT models","buckets":[
                {"bucketId":"y1","window":"5h","remainingFraction":0.0,"resetTime":"2026-07-22T17:47:00Z"}]}
            ]}}"#,
        )
        .unwrap();
        let snap = parse_quota_summary(&v, "Pro".into()).unwrap();
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 50);
        assert_eq!(snap.weekly.as_ref().unwrap().utilization_pct, 10);
        assert_eq!(snap.third_party_session.unwrap().utilization_pct, 100);
        assert!(snap.third_party_weekly.is_none());
    }

    #[test]
    fn duplicate_or_unclassified_buckets_cannot_overwrite_a_slot() {
        let duplicate: serde_json::Value = serde_json::from_str(
            r#"{"response":{"groups":[{"displayName":"Gemini Models","buckets":[
              {"bucketId":"gemini-5h","window":"5h","remainingFraction":0.9},
              {"bucketId":"gemini-5h-copy","window":"5h","remainingFraction":0.1},
              {"bucketId":"gemini-weekly","window":"weekly","remainingFraction":0.8}
            ]}]}}"#,
        )
        .unwrap();
        let err = parse_quota_summary(&duplicate, "Pro".into()).unwrap_err();
        assert!(err.to_string().contains("duplicate Gemini 5h"), "{err}");

        // A future pool or cadence is ignored, not silently treated as the
        // Claude/GPT 5h slot.
        let unrelated: serde_json::Value = serde_json::from_str(
            r#"{"response":{"groups":[
              {"displayName":"Gemini Models","buckets":[
                {"bucketId":"gemini-5h","window":"5h","remainingFraction":0.9},
                {"bucketId":"gemini-weekly","window":"weekly","remainingFraction":0.8},
                {"bucketId":"gemini-monthly","window":"monthly","remainingFraction":0.7}
              ]},
              {"displayName":"Future Models","buckets":[
                {"bucketId":"future-5h","window":"5h","remainingFraction":0.1}
              ]}
            ]}}"#,
        )
        .unwrap();
        let snap = parse_quota_summary(&unrelated, "Pro".into()).unwrap();
        assert!(snap.third_party_session.is_none());
        assert!(snap.third_party_weekly.is_none());
    }

    /// A drifted bucket must fail the parse rather than report a reassuring
    /// "0% used" for a window whose real state is unknown.
    #[test]
    fn a_bucket_without_a_usable_fraction_is_rejected() {
        for bad in [r#""oops""#, "null", "-0.01", "1.01"] {
            let v: serde_json::Value = serde_json::from_str(&format!(
                r#"{{"response":{{"groups":[{{"displayName":"Gemini Models","buckets":[
                  {{"bucketId":"gemini-5h","window":"5h","remainingFraction":{bad}}},
                  {{"bucketId":"gemini-weekly","window":"weekly","remainingFraction":0.9}}]}}]}}}}"#
            ))
            .unwrap();
            let err = parse_quota_summary(&v, "Pro".into()).unwrap_err();
            assert!(err.to_string().contains("gemini-5h"), "{bad}: {err}");
        }
    }

    #[test]
    fn malformed_present_reset_is_rejected_instead_of_disabling_expiry() {
        for bad in [serde_json::json!("not-a-time"), serde_json::json!(42)] {
            let mut v: serde_json::Value = serde_json::from_str(QUOTA_JSON).unwrap();
            v["response"]["groups"][0]["buckets"][0]["resetTime"] = bad;
            let err = parse_quota_summary(&v, "Pro".into()).unwrap_err();
            assert!(err.to_string().contains("resetTime"), "{err}");
        }
    }

    /// The cache must round-trip a product that has no 5h window, and must
    /// still reject a document it did not write whole — an explicit `null`
    /// means "no such window", a missing key means truncation.
    #[test]
    fn a_weekly_only_snapshot_round_trips_through_the_cache() {
        let mut snap = parsed();
        snap.session = None;
        snap.third_party_session = None;

        let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap();
        let back = parse_cache_at(&bytes, None, now()).expect("weekly-only cache is usable");

        assert!(back.session.is_none());
        assert_eq!(
            back.weekly.as_ref().unwrap().utilization_pct,
            snap.weekly.as_ref().unwrap().utilization_pct
        );
    }

    /// Issue #139: Antigravity CLI 1.1.22 on a paid account returns weekly
    /// buckets and no 5-hour ones. Two usable windows arrived, so requiring a
    /// Gemini 5h bucket threw both away and failed the whole vendor. This is
    /// the reporter's payload.
    #[test]
    fn a_product_reporting_only_weekly_buckets_still_renders_them() {
        let summary = serde_json::json!({
            "groups": [
                {
                    "displayName": "Gemini Models",
                    "buckets": [{
                        "bucketId": "gemini-weekly", "window": "weekly",
                        "remainingFraction": 0.42,
                    }],
                },
                {
                    "displayName": "Claude and GPT models",
                    "buckets": [{
                        "bucketId": "3p-weekly", "window": "weekly",
                        "remainingFraction": 0.9,
                    }],
                },
            ],
        });

        let snap = parse_quota_summary(&summary, "Pro".into()).expect("weekly-only is usable");

        assert!(snap.session.is_none(), "no 5h bucket arrived");
        assert!(snap.third_party_session.is_none());
        assert_eq!(snap.weekly.as_ref().unwrap().utilization_pct, 58);
        assert_eq!(
            snap.third_party_weekly.as_ref().unwrap().utilization_pct,
            10
        );
    }

    /// The opposite shape must work for the same reason — the fix is "at least
    /// one window", not "weekly is the required one now".
    #[test]
    fn a_product_reporting_only_five_hour_buckets_still_renders_them() {
        let summary = serde_json::json!({
            "groups": [{
                "displayName": "Gemini Models",
                "buckets": [{
                    "bucketId": "gemini-5h", "window": "5h", "remainingFraction": 0.25,
                }],
            }],
        });

        let snap = parse_quota_summary(&summary, "Pro".into()).expect("5h-only is usable");

        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 75);
        assert!(snap.weekly.is_none());
    }

    /// Nothing recognisable is still an error, and still names what arrived so
    /// the next report is diagnosable.
    #[test]
    fn a_summary_with_no_recognisable_bucket_errors_and_names_what_it_had() {
        let summary = serde_json::json!({
            "groups": [{
                "displayName": "Gemini Models",
                "buckets": [{
                    "bucketId": "gemini-daily", "window": "daily", "remainingFraction": 0.9,
                }],
            }],
        });

        let rendered = parse_quota_summary(&summary, "Pro".into())
            .expect_err("an unrecognised cadence alone is not a snapshot")
            .to_string();

        assert!(
            rendered.contains("no bucket in a window we recognise"),
            "{rendered}"
        );
        assert!(rendered.contains("gemini-daily"), "{rendered}");
        assert!(rendered.contains("window daily"), "{rendered}");
    }

    /// A summary with groups but no buckets at all is a different situation
    /// from a summary whose buckets we did not recognise, and says so.
    #[test]
    fn a_summary_with_no_buckets_at_all_says_that_rather_than_listing_nothing() {
        let summary = serde_json::json!({
            "groups": [{"displayName": "Gemini", "buckets": []}],
        });

        let rendered = parse_quota_summary(&summary, "Pro".into())
            .expect_err("no buckets is an error")
            .to_string();

        assert!(rendered.contains("no buckets at all"), "{rendered}");
        assert!(!rendered.contains("it offered:"), "{rendered}");
    }

    /// An unnamed bucket must still be listed — a summary of nothing but
    /// unnamed buckets is itself the finding.
    #[test]
    fn buckets_without_an_id_are_still_named_in_the_error() {
        let summary = serde_json::json!({
            "groups": [{"displayName": "", "buckets": [{"remainingFraction": 0.5}]}],
        });

        let rendered = parse_quota_summary(&summary, "Pro".into())
            .expect_err("an unusable summary is an error")
            .to_string();

        assert!(rendered.contains("<unnamed>"), "{rendered}");
    }

    #[test]
    fn missing_gemini_buckets_is_an_error_not_a_zero_bar() {
        let v: serde_json::Value = serde_json::from_str(r#"{"response":{"groups":[]}}"#).unwrap();
        assert!(parse_quota_summary(&v, "Pro".into()).is_err());
    }

    #[test]
    fn cache_round_trip_preserves_every_window() {
        let snap = parsed();
        let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap();
        assert_eq!(parse_cache_at(&bytes, None, now()).unwrap(), snap);
    }

    /// A truncated payload must fail so the caller refetches. Defaulting the
    /// missing field to 0 would serve a confident "0% used" for the rest of the
    /// TTL — the fabricated-placeholder defect corrected in PR #26.
    #[test]
    fn a_truncated_cached_payload_is_rejected_not_zeroed() {
        let full = snap_to_json(&parsed());
        for missing in ["session_pct", "weekly_pct"] {
            let mut v = full.clone();
            v.as_object_mut().unwrap().remove(missing);
            let bytes = serde_json::to_vec(&v).unwrap();
            let err = parse_cache_at(&bytes, None, now()).unwrap_err();
            assert!(err.to_string().contains(missing), "{missing}: {err}");
        }
        // A wholly empty object is not a zero-usage snapshot either.
        assert!(parse_cache_at(b"{}", None, now()).is_err());
    }

    #[test]
    fn cached_percentages_are_range_checked_before_narrowing() {
        let full = snap_to_json(&parsed());
        for (key, bad) in [
            ("session_pct", serde_json::json!(-1)),
            ("weekly_pct", serde_json::json!(101)),
            ("session_pct", serde_json::json!(i64::MAX)),
            ("tp_session_pct", serde_json::json!("75")),
        ] {
            let mut v = full.clone();
            v[key] = bad;
            let bytes = serde_json::to_vec(&v).unwrap();
            let err = parse_cache_at(&bytes, None, now()).unwrap_err();
            assert!(err.to_string().contains(key), "{key}: {err}");
        }
    }

    #[test]
    fn malformed_cached_reset_is_rejected_instead_of_served_for_a_week() {
        let mut v = snap_to_json(&parsed());
        v["session_reset"] = serde_json::json!("not-a-time");
        let bytes = serde_json::to_vec(&v).unwrap();
        let err = parse_cache_at(&bytes, None, now()).unwrap_err();
        assert!(err.to_string().contains("session_reset"), "{err}");
    }

    /// With Antigravity closed the cache is served for up to `MAX_STALE`, but a
    /// window whose reset has passed has since rolled over — the real figure is
    /// back near zero while the payload still carries the old one. Serving that
    /// would present a known-obsolete number as current.
    #[test]
    fn a_cache_past_its_reset_is_refused() {
        let bytes = serde_json::to_vec(&snap_to_json(&parsed())).unwrap();
        let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);

        // Before every reset: served.
        assert!(parse_cache_at(&bytes, None, now()).is_ok());
        // One second before the earliest (the two 5h windows, 17:47:00Z).
        assert!(parse_cache_at(&bytes, None, at("2026-07-22T17:46:59Z")).is_ok());

        // The reset instant itself already counts as rolled over.
        let err = parse_cache_at(&bytes, None, at("2026-07-22T17:47:00Z")).unwrap_err();
        assert!(err.to_string().contains("5h"), "{err}");

        // Well past it — this is the reboot-with-nothing-running case.
        assert!(parse_cache_at(&bytes, None, at("2026-07-23T09:00:00Z")).is_err());
    }

    /// The weekly windows outlive the 5-hour ones, so expiry must be reported
    /// per window rather than assuming the shortest one speaks for all four.
    #[test]
    fn expiry_names_the_window_that_rolled_over() {
        let mut snap = parsed();
        // Drop the 5h windows so only the weeklies can expire.
        snap.session.as_mut().unwrap().resets_at = None;
        snap.third_party_session = None;
        let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap();
        let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);

        // Past the 5h resets but before either weekly: still usable.
        assert!(parse_cache_at(&bytes, None, at("2026-07-23T09:00:00Z")).is_ok());

        // Past the Gemini weekly (28th) but not the third-party one (29th).
        let err = parse_cache_at(&bytes, None, at("2026-07-28T18:00:00Z")).unwrap_err();
        assert!(err.to_string().contains("Gemini weekly"), "{err}");
    }

    /// A window with no reset time is unknown, not expired.
    #[test]
    fn a_window_without_a_reset_never_expires() {
        let mut snap = parsed();
        for w in [&mut snap.session, &mut snap.weekly].into_iter().flatten() {
            w.resets_at = None;
        }
        snap.third_party_session = None;
        snap.third_party_weekly = None;
        let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap();
        let far_future = DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(parse_cache_at(&bytes, None, far_future).is_ok());
    }

    /// Switching Google accounts must not show the previous account's quota.
    #[test]
    fn a_cache_from_another_account_is_rejected() {
        let mut snap = parsed();
        snap.account = "acct:aaaa".into();
        let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap();

        assert!(parse_cache_at(&bytes, Some("acct:bbbb"), now()).is_err());
        assert_eq!(
            parse_cache_at(&bytes, Some("acct:aaaa"), now()).unwrap(),
            snap
        );

        // A payload written before the account was recorded is unattributable.
        let mut legacy = snap_to_json(&snap);
        legacy.as_object_mut().unwrap().remove("account");
        let legacy = serde_json::to_vec(&legacy).unwrap();
        assert!(parse_cache_at(&legacy, Some("acct:aaaa"), now()).is_err());
    }

    /// With no local server there is nothing to compare against — and nothing
    /// is consuming quota either, so the last known figures still stand.
    #[test]
    fn an_unverifiable_cache_is_served_rather_than_discarded() {
        let mut snap = parsed();
        snap.account = "acct:aaaa".into();
        let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap();
        assert_eq!(parse_cache_at(&bytes, None, now()).unwrap(), snap);
    }

    #[test]
    fn account_key_fingerprints_rather_than_storing_the_address() {
        let with = |email: &str| account_key(&serde_json::json!({"userStatus": {"email": email}}));
        let a = with("someone@example.com");
        assert!(!a.contains("someone"), "{a}");
        assert!(!a.contains('@'), "{a}");
        assert_eq!(a, with("someone@example.com"), "must be stable");
        assert_ne!(a, with("other@example.com"));
        // An unidentifiable response still compares equal to itself.
        let unknown = account_key(&serde_json::json!({}));
        assert_eq!(unknown, account_key(&serde_json::json!({"userStatus": {}})));
        assert_ne!(unknown, a);
    }

    /// The third-party pool is genuinely optional — a plan without it caches a
    /// null and must still read back, unlike the required Gemini windows.
    #[test]
    fn absent_third_party_windows_are_not_treated_as_corruption() {
        let mut snap = parsed();
        snap.third_party_session = None;
        snap.third_party_weekly = None;
        let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap();
        assert_eq!(parse_cache_at(&bytes, None, now()).unwrap(), snap);
    }

    #[test]
    fn cache_round_trip_preserves_absent_third_party_windows() {
        let mut snap = parsed();
        snap.third_party_session = None;
        snap.third_party_weekly = None;
        let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap();
        assert_eq!(parse_cache_at(&bytes, None, now()).unwrap(), snap);
    }

    #[test]
    fn pct_used_inverts_valid_fractions() {
        assert_eq!(pct_used(1.0), 0);
        assert_eq!(pct_used(0.0), 100);
        assert_eq!(pct_used(0.5), 50);
    }

    #[test]
    fn plan_falls_back_through_the_status_payload() {
        let tier: serde_json::Value =
            serde_json::from_str(r#"{"userStatus":{"userTier":{"name":"Google AI Pro"}}}"#)
                .unwrap();
        assert_eq!(plan_from_status(&tier), "Google AI Pro");

        let plan_only: serde_json::Value = serde_json::from_str(
            r#"{"userStatus":{"planStatus":{"planInfo":{"planName":"Pro"}}}}"#,
        )
        .unwrap();
        assert_eq!(plan_from_status(&plan_only), "Pro");

        let empty: serde_json::Value = serde_json::from_str("{}").unwrap();
        assert_eq!(plan_from_status(&empty), DEFAULT_PLAN);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proc_net_parser_keeps_only_listening_rows() {
        let listen = "   0: 0100007F:975B 00000000:0000 0A 00000000:00000000 \
                      00:00000000 00000000  1000        0 123456 1 0000 100 0";
        assert_eq!(parse_proc_net_line(listen), Some((38747, 123456)));

        let established = "   1: 0100007F:975B 0100007F:A1B2 01 00000000:00000000 \
                           00:00000000 00000000  1000        0 123457 1 0000 100 0";
        assert_eq!(parse_proc_net_line(established), None);

        assert_eq!(parse_proc_net_line("garbage"), None);
    }

    #[test]
    fn explicit_address_comes_first_and_gets_a_scheme() {
        assert_eq!(
            candidate_bases_with(Some("127.0.0.1:1234"), vec![5678]),
            vec![
                "http://127.0.0.1:1234".to_string(),
                "http://127.0.0.1:5678".to_string(),
            ]
        );
        // Trailing slashes are trimmed.
        assert_eq!(
            candidate_bases_with(Some("127.0.0.1:1234/"), vec![5678]),
            vec![
                "http://127.0.0.1:1234".to_string(),
                "http://127.0.0.1:5678".to_string(),
            ]
        );
        // Duplicate base URL is omitted.
        assert_eq!(
            candidate_bases_with(Some("127.0.0.1:5678"), vec![5678]),
            vec!["http://127.0.0.1:5678".to_string()]
        );
        // Duplicate discovered ports are omitted.
        assert_eq!(
            candidate_bases_with(None, vec![5678, 5678]),
            vec!["http://127.0.0.1:5678".to_string()]
        );
        // An address that already carries a scheme is left alone.
        assert_eq!(
            candidate_bases_with(Some("https://host:9"), vec![]),
            vec!["https://host:9".to_string()]
        );
    }

    fn http(status: u16) -> AppError {
        AppError::Http {
            status,
            body: String::new(),
        }
    }

    fn missing_csrf() -> AppError {
        AppError::Http {
            status: 401,
            body: r#"{"code":"unauthenticated","message":"missing CSRF token"}"#.into(),
        }
    }

    #[test]
    fn only_agys_exact_missing_csrf_response_enables_remote_fallback() {
        assert!(is_missing_csrf(&missing_csrf()));
        assert!(!is_missing_csrf(&AppError::Http {
            status: 403,
            body: r#"{"code":"unauthenticated","message":"missing CSRF token"}"#.into(),
        }));
        assert!(!is_missing_csrf(&AppError::Http {
            status: 401,
            body: r#"{"code":"other","message":"missing CSRF token"}"#.into(),
        }));
        assert!(!is_missing_csrf(&AppError::Http {
            status: 401,
            body: "prefix: missing CSRF token".into(),
        }));
    }

    #[test]
    fn a_real_auth_failure_outranks_agys_missing_csrf_response() {
        for errors in [
            vec![missing_csrf(), http(403)],
            vec![http(401), missing_csrf()],
        ] {
            let err = select_probe_error(errors);
            assert!(is_actionable(&err), "{err}");
            assert!(!is_missing_csrf(&err), "{err}");
        }
    }

    #[test]
    fn several_agy_sessions_still_select_the_remote_fallback_reason() {
        let err = select_probe_error(vec![missing_csrf(), missing_csrf()]);
        assert!(is_missing_csrf(&err), "{err}");
        assert!(remote_fallback_reason(&err).is_some(), "{err}");
    }

    /// A signed-out server is worth reporting even when a later candidate only
    /// refused the connection — that is the whole point of probing on past the
    /// first failure.
    #[test]
    fn an_auth_failure_outranks_later_transport_noise() {
        let err = select_probe_error(vec![
            http(401),
            AppError::Transport("connection refused".into()),
        ]);
        assert!(matches!(err, AppError::Http { status: 401, .. }), "{err}");

        let err = select_probe_error(vec![
            AppError::Transport("connection refused".into()),
            http(403),
        ]);
        assert!(matches!(err, AppError::Http { status: 403, .. }), "{err}");
    }

    /// The *first* actionable failure wins, so the explicit override's message
    /// survives a second signed-out product further down the list.
    #[test]
    fn the_first_auth_failure_wins() {
        let err = select_probe_error(vec![http(401), http(403)]);
        assert!(matches!(err, AppError::Http { status: 401, .. }), "{err}");
    }

    /// With nothing actionable, the last failure stands in for "nothing
    /// answered" — and stays transient, so the widget falls back silently
    /// instead of shouting about a product that simply is not running.
    #[test]
    fn without_an_auth_failure_the_last_error_stands() {
        let err = select_probe_error(vec![
            AppError::Transport("first".into()),
            http(500),
            AppError::Transport("last".into()),
        ]);
        assert!(
            matches!(&err, AppError::Transport(m) if m == "last"),
            "{err}"
        );
        assert!(err.is_transient());
    }

    /// A 5xx is a server that answered but broke; the user cannot act on it, so
    /// it must not outrank a later real failure the way a 401 does.
    #[test]
    fn a_server_error_is_not_treated_as_actionable() {
        let err = select_probe_error(vec![http(500), http(401)]);
        assert!(matches!(err, AppError::Http { status: 401, .. }), "{err}");
    }

    #[test]
    fn no_candidates_at_all_yields_a_generic_error() {
        let err = select_probe_error(Vec::new());
        assert!(
            err.to_string().contains("no local server answered"),
            "{err}"
        );
    }

    /// Verbatim from Go's `net/http`: this is what every Antigravity product's
    /// HTTPS listener replies to the cleartext probe, so it is the tail of a
    /// normal discovery run rather than a symptom.
    fn tls_echo() -> AppError {
        AppError::Http {
            status: 400,
            body: "Client sent an HTTP request to an HTTPS server.\n".into(),
        }
    }

    /// The RPC listener is probed first, so whatever it said is the diagnosis.
    /// The TLS port is reached only afterwards and always "fails", so without
    /// demoting it, it overwrites the one error that came from the server the
    /// user actually cares about.
    #[test]
    fn a_tls_echo_does_not_mask_what_the_rpc_listener_said() {
        let err = select_probe_error(vec![
            AppError::Http {
                status: 500,
                body: "GetUserStatus: internal".into(),
            },
            tls_echo(),
        ]);
        assert!(
            matches!(&err, AppError::Http { status: 500, body } if body.contains("internal")),
            "{err}"
        );
    }

    /// The regression that motivated this: an `Http` is not transient, so an
    /// echo standing in as "the last failure" costs the silent cache fallback
    /// that `without_an_auth_failure_the_last_error_stands` exists to protect.
    /// A product that is merely not serving RPC must stay quiet.
    #[test]
    fn a_tls_echo_does_not_cost_the_silent_fallback() {
        let err = select_probe_error(vec![
            AppError::Transport("connection refused".into()),
            tls_echo(),
        ]);
        assert!(
            matches!(&err, AppError::Transport(m) if m == "connection refused"),
            "{err}"
        );
        assert!(
            err.is_transient(),
            "the echo must not make the run non-transient: {err}"
        );
    }

    /// Demoted, not discarded. When the TLS listener is genuinely all that
    /// answered, its reply is still better than a generic "nothing answered".
    #[test]
    fn a_tls_echo_still_stands_when_it_is_the_only_thing_that_answered() {
        let err = select_probe_error(vec![tls_echo()]);
        assert!(matches!(err, AppError::Http { status: 400, .. }), "{err}");
    }

    /// The demotion keys on the body, so the language server's own `400` — a
    /// real complaint about a real request — keeps its normal rank.
    #[test]
    fn a_genuine_bad_request_is_not_mistaken_for_a_tls_echo() {
        let err = select_probe_error(vec![
            AppError::Http {
                status: 400,
                body: "unknown method GetUserStatus".into(),
            },
            tls_echo(),
        ]);
        assert!(
            matches!(&err, AppError::Http { status: 400, body } if body.contains("unknown method")),
            "{err}"
        );
    }

    /// Ranking the echo last must not disturb the top of the order.
    #[test]
    fn an_auth_failure_still_outranks_a_tls_echo() {
        let err = select_probe_error(vec![tls_echo(), http(401)]);
        assert!(matches!(err, AppError::Http { status: 401, .. }), "{err}");
    }

    #[test]
    fn every_discovered_port_is_probed_in_order() {
        assert_eq!(
            candidate_bases_with(None, vec![33875, 37435]),
            vec![
                "http://127.0.0.1:33875".to_string(),
                "http://127.0.0.1:37435".to_string(),
            ]
        );
    }

    /// The server's port is drawn from the ephemeral range, so there is nothing
    /// sensible to guess when discovery comes up empty. Probing a hardcoded
    /// port would contact an unrelated process; callers get the "start
    /// Antigravity or set ANTIGRAVITY_LS_ADDRESS" error instead.
    #[test]
    fn empty_discovery_yields_no_candidates() {
        assert!(candidate_bases_with(None, vec![]).is_empty());
        assert!(candidate_bases_with(Some(""), vec![]).is_empty());
    }

    #[test]
    fn every_antigravity_product_is_recognised() {
        // Antigravity 2.0 / IDE: a separate language_server child.
        assert!(is_antigravity_process(
            "language_server\n",
            Some("/opt/antigravity/resources/bin/language_server")
        ));
        // agy CLI: embeds the RPC surface in its own process.
        assert!(is_antigravity_process(
            "agy\n",
            Some("/home/u/.local/bin/agy")
        ));
        // Recognised by path even when the process name says nothing.
        assert!(is_antigravity_process(
            "node",
            Some("/opt/antigravity/bin/helper")
        ));
        assert!(is_antigravity_process("antigravity", None));
        assert!(is_antigravity_process("agy.exe", None));
        assert!(is_antigravity_process("Antigravity.exe", None));
        assert!(is_antigravity_process("language_server.exe", None));
        assert!(is_antigravity_process(
            "language_server_windows_x64.exe",
            None
        ));
        assert!(is_antigravity_process(
            "node.exe",
            Some(r"C:\Users\u\AppData\Local\agy.exe")
        ));
    }

    #[test]
    fn unrelated_processes_are_not_probed() {
        assert!(!is_antigravity_process("sshd", Some("/usr/sbin/sshd")));
        assert!(!is_antigravity_process("node", Some("/usr/bin/node")));
        // "legacy" ends in a substring of "/agy" but is not the CLI.
        assert!(!is_antigravity_process("legacy", Some("/usr/bin/legacy")));
        assert!(!is_antigravity_process("legacy.exe", None));
        assert!(!is_antigravity_process("not-agy.exe", None));
        assert!(!is_antigravity_process("", None));
    }

    #[test]
    fn windows_process_names_decode_until_nul_and_tolerate_invalid_utf16() {
        let mut raw: Vec<u16> = "agy.exe".encode_utf16().collect();
        raw.extend([0, b'x' as u16]);
        assert_eq!(decode_windows_process_name(&raw), "agy.exe");
        assert_eq!(decode_windows_process_name(&[0xd800]), "�");
        assert_eq!(decode_windows_process_name(&[]), "");
    }

    #[test]
    fn windows_process_filter_keeps_only_antigravity_pids() {
        let processes = vec![
            (10, "agy.exe".to_string()),
            (20, "language_server_windows_x64.exe".to_string()),
            (30, "sshd.exe".to_string()),
        ];
        let pids = matching_windows_process_ids(&processes);
        assert_eq!(pids, std::collections::HashSet::from([10, 20]));
    }

    #[test]
    fn windows_listener_filter_joins_pid_loopback_and_port() {
        let pids = std::collections::HashSet::from([10]);
        let rows = [
            WindowsTcpRow {
                local_addr: [127, 0, 0, 1],
                local_port: u32::from(59870u16.to_be()),
                pid: 10,
            },
            WindowsTcpRow {
                local_addr: [127, 0, 0, 1],
                local_port: u32::from(59868u16.to_be()),
                pid: 10,
            },
            WindowsTcpRow {
                local_addr: [127, 0, 0, 1],
                local_port: u32::from(59870u16.to_be()),
                pid: 10,
            },
            WindowsTcpRow {
                local_addr: [0, 0, 0, 0],
                local_port: u32::from(50000u16.to_be()),
                pid: 10,
            },
            WindowsTcpRow {
                local_addr: [127, 0, 0, 1],
                local_port: u32::from(50001u16.to_be()),
                pid: 99,
            },
            WindowsTcpRow {
                local_addr: [127, 0, 0, 1],
                local_port: 0,
                pid: 10,
            },
        ];
        assert_eq!(matching_windows_ports(&pids, &rows), vec![59870, 59868]);
    }

    /// Antigravity 2.0 and an interactive `agy` session at once. Their port
    /// pairs must not be flattened into one set: sorting all four descending
    /// would put pid 20's TLS listener ahead of pid 10's RPC listener.
    #[test]
    fn windows_ports_from_two_products_keep_tls_listeners_last() {
        let pids = std::collections::HashSet::from([10, 20]);
        let row = |port: u16, pid: u32| WindowsTcpRow {
            local_addr: [127, 0, 0, 1],
            local_port: u32::from(port.to_be()),
            pid,
        };
        let rows = [
            row(40000, 10),
            row(40001, 10),
            row(50000, 20),
            row(50001, 20),
        ];
        assert_eq!(
            matching_windows_ports(&pids, &rows),
            vec![40001, 50001, 40000, 50000]
        );
    }

    /// The high-to-low preference only means something per product, so the
    /// grouping is what keeps a second product's TLS listener from being
    /// probed before the first product's RPC listener.
    #[test]
    fn probe_order_puts_every_rpc_listener_ahead_of_every_tls_listener() {
        use std::collections::BTreeMap;

        // One process, the ordinary case: RPC (higher) before TLS (lower).
        assert_eq!(
            probe_order(BTreeMap::from([(10, vec![59868, 59870])])),
            vec![59870, 59868]
        );
        // Two products. A plain descending sort would yield 50001, 50000,
        // 40001, 40000 and reach pid 20's TLS listener second; taking the
        // ports rank by rank leaves both TLS listeners at the back, where they
        // are touched only if no RPC listener answered.
        assert_eq!(
            probe_order(BTreeMap::from([
                (10, vec![40000, 40001]),
                (20, vec![50000, 50001]),
            ])),
            vec![40001, 50001, 40000, 50000]
        );
        // Uneven groups: the extra port of the deeper group trails everything
        // it ranks below, and a port claimed by two pids is probed once.
        assert_eq!(
            probe_order(BTreeMap::from([
                (10, vec![6000, 5000, 4000]),
                (20, vec![6000, 7000]),
            ])),
            vec![6000, 7000, 5000, 4000]
        );
        assert!(probe_order(BTreeMap::new()).is_empty());
    }

    /// A dual-stack bind names the same port from both `/proc/net/tcp` and
    /// `tcp6`. Those rows are one listener, so they must not consume two ranks
    /// and push the product's real second listener down past another
    /// product's.
    #[test]
    fn a_port_named_twice_by_one_product_still_occupies_one_rank() {
        use std::collections::BTreeMap;

        assert_eq!(
            probe_order(BTreeMap::from([
                (10, vec![40001, 40001, 40000, 40000]),
                (20, vec![50001, 50000]),
            ])),
            vec![40001, 50001, 40000, 50000],
            "duplicate rows must not reorder the ranks below them"
        );
    }

    /// The ordering rests on each product showing both listeners. A product
    /// caught mid-startup, with only its TLS port bound, sits alone at rank 0
    /// and is probed first — documented as the known cost, and harmless
    /// because every candidate is probed anyway.
    #[test]
    fn a_half_started_product_is_the_documented_exception() {
        use std::collections::BTreeMap;

        assert_eq!(
            probe_order(BTreeMap::from([
                (10, vec![40000]),
                (20, vec![50001, 50000])
            ])),
            vec![40000, 50001, 50000]
        );
    }

    /// `ANTIGRAVITY_LS_ADDRESS` is user input. An entry that leaves no
    /// authority to connect to is dropped instead of probed, so it can neither
    /// spend a round-trip nor add a failure that competes with the real one in
    /// [`select_probe_error`].
    #[test]
    fn an_override_with_no_authority_is_dropped_not_probed() {
        for junk in ["/", "///", "http://", "https://", "  /  "] {
            assert_eq!(
                candidate_bases_with(Some(junk), vec![4242]),
                vec!["http://127.0.0.1:4242".to_string()],
                "{junk:?} should not survive as a candidate"
            );
        }
        assert!(candidate_bases_with(Some("/"), vec![]).is_empty());
    }

    #[test]
    fn windows_table_bounds_reject_truncation_and_overflow() {
        assert_eq!(checked_windows_row_count(52, 4, 24, 2), Some(2));
        assert_eq!(checked_windows_row_count(51, 4, 24, 2), None);
        assert_eq!(checked_windows_row_count(4, 4, 24, 0), Some(0));
        assert_eq!(checked_windows_row_count(52, 4, 0, 2), None);
        assert_eq!(
            checked_windows_row_count(usize::MAX, 4, 24, usize::MAX),
            None
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_tcp_table_parser_copies_complete_rows_only() {
        use std::mem::{offset_of, size_of};
        use windows_sys::Win32::NetworkManagement::IpHelper::{
            MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
        };

        let offset = offset_of!(MIB_TCPTABLE_OWNER_PID, table);
        let used = offset + 2 * size_of::<MIB_TCPROW_OWNER_PID>();
        let words = used.div_ceil(size_of::<u32>());
        let mut buffer = vec![0u32; words];
        let first = MIB_TCPROW_OWNER_PID {
            dwLocalAddr: u32::from_ne_bytes([127, 0, 0, 1]),
            dwLocalPort: u32::from(59868u16.to_be()),
            dwOwningPid: 10,
            ..Default::default()
        };
        let second = MIB_TCPROW_OWNER_PID {
            dwLocalAddr: u32::from_ne_bytes([127, 0, 0, 1]),
            dwLocalPort: u32::from(59870u16.to_be()),
            dwOwningPid: 10,
            ..Default::default()
        };
        unsafe {
            buffer.as_mut_ptr().write_unaligned(2);
            let rows = buffer
                .as_mut_ptr()
                .cast::<u8>()
                .add(offset)
                .cast::<MIB_TCPROW_OWNER_PID>();
            rows.write_unaligned(first);
            rows.add(1).write_unaligned(second);
        }

        assert_eq!(
            parse_windows_tcp_rows(&buffer, used),
            vec![
                WindowsTcpRow {
                    local_addr: [127, 0, 0, 1],
                    local_port: u32::from(59868u16.to_be()),
                    pid: 10,
                },
                WindowsTcpRow {
                    local_addr: [127, 0, 0, 1],
                    local_port: u32::from(59870u16.to_be()),
                    pid: 10,
                },
            ]
        );
        assert!(parse_windows_tcp_rows(&buffer, used - 1).is_empty());
    }

    #[test]
    fn lsof_parser_keeps_only_ports_owned_by_antigravity_processes() {
        // `agy` (pid 74101) has three listening sockets; `sshd` (pid 200) has
        // one that must be excluded even though it sorts right after `c`.
        let output = "p74101\ncagy\nf10\nn127.0.0.1:8829\nf11\nn127.0.0.1:61289\nf12\nn127.0.0.1:61290\np200\ncsshd\nf5\nn*:22\n";
        assert_eq!(parse_lsof_pcn(output), vec![61290, 61289, 8829]);
    }

    /// The pid on each `p` line has to survive to the `n` lines, or the ports
    /// of two running products collapse into one group and rank ordering can
    /// no longer keep the TLS listeners last.
    #[test]
    fn lsof_parser_keeps_each_products_ports_in_its_own_group() {
        let output = concat!(
            "p100\ncagy\nf3\nn127.0.0.1:40000\nf4\nn127.0.0.1:40001\n",
            "p200\nclanguage_server\nf5\nn127.0.0.1:50000\nf6\nn127.0.0.1:50001\n",
        );
        assert_eq!(
            parse_lsof_pcn(output),
            vec![40001, 50001, 40000, 50000],
            "both HTTP listeners must precede both TLS listeners"
        );
    }

    #[test]
    fn lsof_parser_matches_the_capitalised_macos_app_name() {
        let output = "p900\ncAntigravity\nf7\nn127.0.0.1:54321\n";
        assert_eq!(parse_lsof_pcn(output), vec![54321]);
    }

    #[test]
    fn lsof_parser_deduplicates_and_handles_empty_output() {
        let output = "p1\ncagy\nf3\nn127.0.0.1:9000\nf4\nn127.0.0.1:9000\n";
        assert_eq!(parse_lsof_pcn(output), vec![9000]);
        assert!(parse_lsof_pcn("").is_empty());
    }

    /// First run with Antigravity closed: no cache to serve, so the user must
    /// be told what to start — not "no usable cache", which says nothing.
    #[test]
    fn missing_cache_surfaces_the_diagnosis_not_a_cache_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().join("usage.json"));
        let reason = AppError::Credentials("Antigravity: no local language server found".into());

        let err = fallback_with_error(&cache, None, reason, now()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no local language server found"), "{msg}");
        assert!(!msg.contains("no usable cache"), "{msg}");
    }

    #[test]
    fn unusable_cache_does_not_replace_the_live_diagnosis() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().join("antigravity"));
        cache.write_payload(b"{}").unwrap();

        let reason = AppError::Credentials("Antigravity must be running".into());
        let err = fallback_with_error(&cache, None, reason, now()).unwrap_err();
        assert!(err.to_string().contains("must be running"), "{err}");

        let original = AppError::Transport("original loopback failure".into());
        let err = fallback_silent(&cache, now(), original).unwrap_err();
        assert!(
            err.to_string().contains("original loopback failure"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn rpc_error_bodies_are_bounded_too() {
        let mut server = mockito::Server::new_async().await;
        let path = format!("/{STATUS_RPC}");
        server
            .mock("POST", path.as_str())
            .with_status(500)
            .with_body("x".repeat(crate::vendor::MAX_BODY_BYTES + 1))
            .create_async()
            .await;

        let err = post_rpc(&reqwest::Client::new(), &server.url(), None, STATUS_RPC)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("exceeds"), "{err}");
    }

    #[test]
    fn blank_override_falls_through_to_discovery() {
        assert_eq!(
            candidate_bases_with(Some("   "), vec![4242]),
            vec!["http://127.0.0.1:4242".to_string()]
        );
    }

    // -----------------------------------------------------------------------
    // Remote fallback
    // -----------------------------------------------------------------------

    fn fixture() -> (tempfile::TempDir, Cache) {
        let td = tempfile::TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("antigravity"));
        (td, cache)
    }

    fn endpoints(server: &mockito::Server) -> cloud::Endpoints {
        let base = server.url();
        cloud::Endpoints {
            quota: vec![format!("{base}/daily/quota"), format!("{base}/prod/quota")],
            load_code_assist: vec![format!("{base}/daily/plan"), format!("{base}/prod/plan")],
            token: format!("{base}/token"),
        }
    }

    /// A keyring blob as Antigravity writes it, expiring at `expiry`.
    fn keyring_blob(expiry: &str, with_refresh: bool) -> String {
        let mut token = serde_json::json!({
            "access_token": "KEYRING-AT",
            "expiry": expiry,
        });
        if with_refresh {
            token["refresh_token"] = serde_json::json!("KEYRING-RT");
        }
        serde_json::json!({ "token": token }).to_string()
    }

    /// Well before the fixture's `now()`.
    const EXPIRED: &str = "2026-07-22T11:00:00Z";
    /// Comfortably after it.
    const VALID: &str = "2026-07-22T13:00:00Z";

    /// No local server, this blob, these endpoints.
    fn remote<'a>(blob: &'a str, eps: &'a cloud::Endpoints) -> RemoteOverride<'a> {
        RemoteOverride {
            credential: SavedCredential::Blob(blob),
            endpoints: Some(eps),
            local_bases: Some(vec![]),
        }
    }

    /// The bare summary the API returns: the RPC's payload without its
    /// `{"response": …}` envelope.
    fn bare_summary() -> String {
        let v: serde_json::Value = serde_json::from_str(QUOTA_JSON).unwrap();
        v["response"].to_string()
    }

    fn quota_mock(server: &mut mockito::Server, bearer: &str) -> mockito::Mock {
        server
            .mock("POST", "/daily/quota")
            .match_header("authorization", format!("Bearer {bearer}").as_str())
            .with_status(200)
            .with_body(bare_summary())
    }

    fn token_mock(server: &mut mockito::Server) -> mockito::Mock {
        server
            .mock("POST", "/token")
            .with_status(200)
            .with_body(r#"{"access_token":"NEW-AT","expires_in":3600}"#)
    }

    fn test_oauth() -> cloud::OauthClient {
        cloud::OauthClient {
            id: "test-client".into(),
            secret: "test-client-secret".into(),
        }
    }

    async fn run(cache: &Cache, remote: RemoteOverride<'_>, ttl: Duration) -> Result<FetchOutcome> {
        fetch_snapshot_at(
            &reqwest::Client::new(),
            cache,
            ttl,
            Some(&test_oauth()),
            remote,
            now(),
        )
        .await
    }

    #[tokio::test]
    async fn with_no_local_server_the_saved_session_answers_from_the_api() {
        let mut server = mockito::Server::new_async().await;
        let eps = endpoints(&server);
        let quota = quota_mock(&mut server, "KEYRING-AT")
            .expect(1)
            .create_async()
            .await;
        let plan = server
            .mock("POST", "/daily/plan")
            .match_header("authorization", "Bearer KEYRING-AT")
            .with_status(200)
            .with_body(r#"{"currentTier":{"name":"google_ai_pro"}}"#)
            .expect(1)
            .create_async()
            .await;
        // A token this fresh is used as it is.
        let token = token_mock(&mut server).expect(0).create_async().await;
        let blob = keyring_blob(VALID, true);
        let (_td, cache) = fixture();

        let outcome = run(&cache, remote(&blob, &eps), Duration::from_secs(60))
            .await
            .expect("remote path yields a snapshot");

        quota.assert_async().await;
        plan.assert_async().await;
        token.assert_async().await;
        let snap = outcome.snapshot;
        assert!(!outcome.stale);
        assert_eq!(snap.source, AntigravitySource::Remote);
        assert_eq!(snap.plan, "Pro");
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 43);
        assert_eq!(snap.weekly.as_ref().unwrap().utilization_pct, 8);
        assert_eq!(snap.third_party_session.unwrap().utilization_pct, 75);
        assert_eq!(snap.third_party_weekly.unwrap().utilization_pct, 0);
        let fingerprint = credential::parse_keyring_blob(&blob).unwrap().fingerprint;
        assert_eq!(snap.account, format!("acct:{fingerprint}"));
        assert!(!snap.account.contains("KEYRING"), "{}", snap.account);

        // What was cached is attributed to the remote path too.
        let cached = parse_cache_at(
            &cache
                .fresh_payload(Duration::from_secs(60))
                .unwrap()
                .unwrap(),
            None,
            now(),
        )
        .unwrap();
        assert_eq!(cached.source, AntigravitySource::Remote);
        assert_eq!(cached.account, snap.account);
    }

    /// `agy` exposes the local RPC port but, unlike the desktop products, does
    /// not expose the CSRF token needed to use it. That one precise response is
    /// equivalent to having no usable local source, so the saved session may
    /// answer without weakening the normal signed-out-server rule below.
    #[tokio::test]
    async fn agys_missing_csrf_response_uses_the_saved_session() {
        let mut server = mockito::Server::new_async().await;
        let eps = endpoints(&server);
        let root = server
            .mock("GET", "/")
            .with_status(404)
            .expect(1)
            .create_async()
            .await;
        let status_path = format!("/{STATUS_RPC}");
        let status = server
            .mock("POST", status_path.as_str())
            .with_status(401)
            .with_body(r#"{"code":"unauthenticated","message":"missing CSRF token"}"#)
            .expect(1)
            .create_async()
            .await;
        let quota = quota_mock(&mut server, "KEYRING-AT")
            .expect(1)
            .create_async()
            .await;
        let plan = server
            .mock("POST", "/daily/plan")
            .match_header("authorization", "Bearer KEYRING-AT")
            .with_status(200)
            .with_body(r#"{"currentTier":{"name":"google_ai_pro"}}"#)
            .expect(1)
            .create_async()
            .await;
        let blob = keyring_blob(VALID, true);
        let (_td, cache) = fixture();

        let outcome = run(
            &cache,
            RemoteOverride {
                credential: SavedCredential::Blob(&blob),
                endpoints: Some(&eps),
                local_bases: Some(vec![server.url()]),
            },
            Duration::ZERO,
        )
        .await
        .expect("saved session bypasses agy's unusable local RPC");

        root.assert_async().await;
        status.assert_async().await;
        quota.assert_async().await;
        plan.assert_async().await;
        assert_eq!(outcome.snapshot.source, AntigravitySource::Remote);
    }

    #[tokio::test]
    async fn agys_missing_csrf_without_a_saved_session_explains_both_options() {
        let mut server = mockito::Server::new_async().await;
        let eps = endpoints(&server);
        let root = server
            .mock("GET", "/")
            .with_status(404)
            .expect(1)
            .create_async()
            .await;
        let status_path = format!("/{STATUS_RPC}");
        let status = server
            .mock("POST", status_path.as_str())
            .with_status(401)
            .with_body(r#"{"code":"unauthenticated","message":"missing CSRF token"}"#)
            .expect(1)
            .create_async()
            .await;
        let quota = quota_mock(&mut server, "KEYRING-AT")
            .expect(0)
            .create_async()
            .await;
        let (_td, cache) = fixture();

        let error = run(
            &cache,
            RemoteOverride {
                credential: SavedCredential::Absent,
                endpoints: Some(&eps),
                local_bases: Some(vec![server.url()]),
            },
            Duration::ZERO,
        )
        .await
        .expect_err("neither the local RPC nor a saved session is usable");

        root.assert_async().await;
        status.assert_async().await;
        quota.assert_async().await;
        let message = error.to_string();
        assert!(message.contains("requires a CSRF token"), "{message}");
        assert!(message.contains("saved Google session"), "{message}");
    }

    /// The refreshed token is persisted under the session's fingerprint, so
    /// the next poll spends no round-trip on the same refresh.
    #[tokio::test]
    async fn an_expired_keyring_token_is_refreshed_once_and_the_refresh_is_reused() {
        let mut server = mockito::Server::new_async().await;
        let eps = endpoints(&server);
        let _quota = quota_mock(&mut server, "NEW-AT")
            .expect(2)
            .create_async()
            .await;
        let token = token_mock(&mut server).expect(1).create_async().await;
        let blob = keyring_blob(EXPIRED, true);
        let (_td, cache) = fixture();

        let first = run(&cache, remote(&blob, &eps), Duration::ZERO)
            .await
            .expect("refresh, then quota");
        assert_eq!(first.snapshot.source, AntigravitySource::Remote);
        token.assert_async().await;

        let fingerprint = credential::parse_keyring_blob(&blob).unwrap().fingerprint;
        let persisted = cloud::read_persisted(&cloud::oauth_cache_path(&cache), &fingerprint)
            .expect("refreshed token persisted under the keyring session's fingerprint");
        assert_eq!(persisted.access_token, "NEW-AT");

        // A zero TTL forces the network again; the token endpoint stays at one hit.
        run(&cache, remote(&blob, &eps), Duration::ZERO)
            .await
            .expect("persisted token reused");
        token.assert_async().await;
    }

    /// The API rejecting a token that was not just minted is the token being
    /// stale, not the session: one refresh, one retry.
    #[tokio::test]
    async fn a_stale_token_the_api_rejects_gets_one_refresh_and_one_retry() {
        let mut server = mockito::Server::new_async().await;
        let eps = endpoints(&server);
        let rejected = server
            .mock("POST", "/daily/quota")
            .match_header("authorization", "Bearer KEYRING-AT")
            .with_status(401)
            .expect(1)
            .create_async()
            .await;
        let accepted = quota_mock(&mut server, "NEW-AT")
            .expect(1)
            .create_async()
            .await;
        let token = token_mock(&mut server).expect(1).create_async().await;
        let blob = keyring_blob(VALID, true);
        let (_td, cache) = fixture();

        let outcome = run(&cache, remote(&blob, &eps), Duration::ZERO)
            .await
            .expect("retry with the refreshed token succeeds");

        rejected.assert_async().await;
        accepted.assert_async().await;
        token.assert_async().await;
        assert_eq!(outcome.snapshot.source, AntigravitySource::Remote);
    }

    /// A refresh Google refuses is the session being gone. The error is
    /// actionable and carries no token material.
    #[tokio::test]
    async fn a_refused_refresh_is_a_credentials_error_without_the_token() {
        let mut server = mockito::Server::new_async().await;
        let eps = endpoints(&server);
        let _token = server
            .mock("POST", "/token")
            .with_status(400)
            .with_body(r#"{"error":"invalid_grant"}"#)
            .create_async()
            .await;
        let blob = keyring_blob(EXPIRED, true);
        let (_td, cache) = fixture();

        let err = run(&cache, remote(&blob, &eps), Duration::ZERO)
            .await
            .expect_err("no cache to fall back on");

        assert!(matches!(err, AppError::Credentials(_)), "{err}");
        let rendered = err.to_string();
        assert!(!rendered.contains("KEYRING-RT"), "{rendered}");
        assert!(!rendered.contains("KEYRING-AT"), "{rendered}");
    }

    /// A `401` against a token minted a moment ago is Google's verdict on the
    /// session; refreshing again would only repeat it.
    #[tokio::test]
    async fn a_rejection_right_after_a_refresh_asks_to_sign_in_again() {
        let mut server = mockito::Server::new_async().await;
        let eps = endpoints(&server);
        let _quota = server
            .mock("POST", "/daily/quota")
            .with_status(401)
            .create_async()
            .await;
        let token = token_mock(&mut server).expect(1).create_async().await;
        let blob = keyring_blob(EXPIRED, true);
        let (_td, cache) = fixture();

        let err = run(&cache, remote(&blob, &eps), Duration::ZERO)
            .await
            .expect_err("rejected after refresh");

        token.assert_async().await;
        assert!(matches!(err, AppError::Credentials(_)), "{err}");
        assert!(err.to_string().contains("sign in again"), "{err}");
    }

    /// A session saved without a refresh token cannot be renewed here.
    #[tokio::test]
    async fn an_expired_session_without_a_refresh_token_asks_to_sign_in_again() {
        let mut server = mockito::Server::new_async().await;
        let eps = endpoints(&server);
        let token = token_mock(&mut server).expect(0).create_async().await;
        let blob = keyring_blob(EXPIRED, false);
        let (_td, cache) = fixture();

        let err = run(&cache, remote(&blob, &eps), Duration::ZERO)
            .await
            .expect_err("nothing to refresh with");

        token.assert_async().await;
        assert!(matches!(err, AppError::Credentials(_)), "{err}");
        assert!(err.to_string().contains("sign in again"), "{err}");
    }

    /// Nothing running and nothing saved: the local diagnosis stands, plus
    /// the one thing the user can now do about it.
    #[tokio::test]
    async fn no_saved_session_extends_the_no_local_server_error() {
        let mut server = mockito::Server::new_async().await;
        let eps = endpoints(&server);
        let quota = quota_mock(&mut server, "KEYRING-AT")
            .expect(0)
            .create_async()
            .await;
        let (_td, cache) = fixture();

        let err = run(
            &cache,
            RemoteOverride {
                credential: SavedCredential::Absent,
                endpoints: Some(&eps),
                local_bases: Some(vec![]),
            },
            Duration::ZERO,
        )
        .await
        .expect_err("no source at all");

        quota.assert_async().await;
        assert!(matches!(err, AppError::Credentials(_)), "{err}");
        let rendered = err.to_string();
        assert!(rendered.contains("no local server found"), "{rendered}");
        assert!(rendered.contains("saved Google session"), "{rendered}");
    }

    /// A local server that is up but signed out is its own diagnosis; the
    /// saved session must not paper over it.
    #[tokio::test]
    async fn a_signed_out_local_server_is_reported_rather_than_bypassed() {
        let mut server = mockito::Server::new_async().await;
        let eps = endpoints(&server);
        let status_path = format!("/{STATUS_RPC}");
        let _status = server
            .mock("POST", status_path.as_str())
            .with_status(401)
            .create_async()
            .await;
        let quota = quota_mock(&mut server, "KEYRING-AT")
            .expect(0)
            .create_async()
            .await;
        let blob = keyring_blob(VALID, true);
        let (_td, cache) = fixture();

        let err = run(
            &cache,
            RemoteOverride {
                credential: SavedCredential::Blob(&blob),
                endpoints: Some(&eps),
                local_bases: Some(vec![server.url()]),
            },
            Duration::ZERO,
        )
        .await
        .expect_err("signed-out local server");

        quota.assert_async().await;
        assert!(matches!(err, AppError::Http { status: 401, .. }), "{err}");
    }

    /// The remote path falls back exactly like the local one: the last good
    /// payload is served stale, with the failure attached.
    #[tokio::test]
    async fn a_cached_remote_snapshot_is_served_when_the_api_fails() {
        let mut server = mockito::Server::new_async().await;
        let eps = endpoints(&server);
        for path in ["/daily/quota", "/prod/quota"] {
            server
                .mock("POST", path)
                .with_status(500)
                .with_body("boom")
                .create_async()
                .await;
        }
        let blob = keyring_blob(VALID, true);
        let (_td, cache) = fixture();
        cache.ensure_dir().unwrap();
        let mut earlier = parsed();
        earlier.source = AntigravitySource::Remote;
        earlier.account = "acct:earlier".into();
        cache
            .write_payload(&serde_json::to_vec(&snap_to_json(&earlier)).unwrap())
            .unwrap();

        let outcome = run(&cache, remote(&blob, &eps), Duration::ZERO)
            .await
            .expect("stale cache stands in");

        assert!(outcome.stale);
        assert_eq!(outcome.snapshot, earlier);
        assert!(
            matches!(outcome.last_error, Some((500, _))),
            "{:?}",
            outcome.last_error
        );
    }

    #[test]
    fn source_round_trips_through_the_cache_and_defaults_to_local() {
        let mut snap = parsed();
        snap.source = AntigravitySource::Remote;
        let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap();
        assert_eq!(
            parse_cache_at(&bytes, None, now()).unwrap().source,
            AntigravitySource::Remote
        );

        // A payload from before the field existed is a local one.
        let mut legacy = snap_to_json(&snap);
        legacy.as_object_mut().unwrap().remove("source");
        let legacy = serde_json::to_vec(&legacy).unwrap();
        assert_eq!(
            parse_cache_at(&legacy, None, now()).unwrap().source,
            AntigravitySource::Local
        );
    }
}
