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

use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::cache::{Cache, MAX_STALE, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::{AntigravitySnapshot, UsageWindow};

const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

const QUOTA_RPC: &str = "exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const STATUS_RPC: &str = "exa.language_server_pb.LanguageServerService/GetUserStatus";

const DEFAULT_PLAN: &str = "Antigravity";

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub snapshot: AntigravitySnapshot,
    pub stale: bool,
    pub last_error: Option<(u16, String)>,
    pub cache_age: Option<Duration>,
}

impl From<FetchOutcome> for crate::vendor::VendorOutcome {
    fn from(o: FetchOutcome) -> Self {
        Self {
            snapshot: crate::usage::VendorSnapshot::Antigravity(o.snapshot),
            stale: o.stale,
            last_error: o.last_error,
            cache_age: o.cache_age,
        }
    }
}

pub async fn fetch_snapshot(
    client: &reqwest::Client,
    cache: &Cache,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;

    // Resolve the signed-in account first so a fresh cache can be attributed.
    // Unlike Grok — where the same check would cost a remote round-trip on
    // every poll — this is loopback, and it is the call that would supply the
    // plan name anyway, so verification is effectively free.
    let session = open_session(client).await;
    let account = session.as_ref().ok().map(|s| s.account.as_str());

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse_cache(bytes, cache, false, account)
    {
        return Ok(outcome);
    }

    match fetch_live(client, session).await {
        Ok(snap) => {
            let bytes = serde_json::to_vec(&snap_to_json(&snap))?;
            cache.write_payload(&bytes)?;
            Ok(FetchOutcome {
                snapshot: snap,
                stale: false,
                last_error: None,
                cache_age: Some(Duration::ZERO),
            })
        }
        Err(e) if e.is_transient() => fallback_silent(cache),
        Err(AppError::Http { status, body }) => {
            cache.mark_stale();
            cache.write_last_error(status, &body);
            let reason = AppError::Http {
                status,
                body: body.clone(),
            };
            fallback_with_error(cache, Some((status, body)), reason)
        }
        Err(e) => {
            cache.mark_stale();
            cache.write_last_error(0, &e.to_string());
            let last_error = Some((0, e.to_string()));
            fallback_with_error(cache, last_error, e)
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

/// Walk every candidate language server until one identifies itself. A machine
/// can host more than one — the desktop app, the IDE and an interactive `agy`
/// session each run their own — and only some of them are signed in.
async fn open_session(client: &reqwest::Client) -> Result<Session> {
    let bases = candidate_bases();
    if bases.is_empty() {
        return Err(AppError::Credentials(
            "Antigravity: no local server found. Quota is only served while Antigravity is \
             running — open the Antigravity app, or an interactive `agy` session, or point \
             ANTIGRAVITY_LS_ADDRESS at a host:port."
                .into(),
        ));
    }

    let mut last_err = None;
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
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| {
        AppError::Other("antigravity: no local server answered GetUserStatus".into())
    }))
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

/// The Antigravity 2.0 server embeds a CSRF token in the HTML it serves at `/`
/// and rejects the RPC without it. The `agy` CLI serves no such page — it 404s
/// at `/` and answers the RPC unauthenticated — so a missing token is not an
/// error here, just a server that does not use one.
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
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }

    let bytes = crate::vendor::read_body_capped(resp, crate::vendor::MAX_BODY_BYTES).await?;
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
pub fn parse_quota_summary(v: &serde_json::Value, plan: String) -> Result<AntigravitySnapshot> {
    let groups = v["response"]["groups"]
        .as_array()
        .or_else(|| v["groups"].as_array())
        .ok_or_else(|| AppError::Other("antigravity: quota summary has no groups".into()))?;

    let mut gemini_5h = None;
    let mut gemini_weekly = None;
    let mut tp_5h = None;
    let mut tp_weekly = None;

    for group in groups {
        let group_name = group["displayName"].as_str().unwrap_or_default();
        let Some(buckets) = group["buckets"].as_array() else {
            continue;
        };
        for bucket in buckets {
            let id = bucket["bucketId"].as_str().unwrap_or_default();
            let window = bucket["window"].as_str().unwrap_or_default();
            let is_weekly = id.ends_with("weekly") || window == "weekly";
            let is_gemini = if id.starts_with("gemini") {
                true
            } else if id.starts_with("3p") {
                false
            } else {
                group_name.contains("Gemini")
            };

            let slot = match (is_gemini, is_weekly) {
                (true, false) => &mut gemini_5h,
                (true, true) => &mut gemini_weekly,
                (false, false) => &mut tp_5h,
                (false, true) => &mut tp_weekly,
            };
            *slot = Some(usage_window(bucket, is_weekly)?);
        }
    }

    let session = gemini_5h.ok_or_else(|| {
        AppError::Other("antigravity: quota summary has no Gemini 5h bucket".into())
    })?;
    let weekly = gemini_weekly.ok_or_else(|| {
        AppError::Other("antigravity: quota summary has no Gemini weekly bucket".into())
    })?;

    Ok(AntigravitySnapshot {
        plan,
        // Stamped by the caller, which is what knows the session's identity.
        account: String::new(),
        session,
        weekly,
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
        .filter(|f| f.is_finite())
        .ok_or_else(|| {
            AppError::Other(format!(
                "antigravity: bucket {} has no finite remainingFraction",
                bucket["bucketId"].as_str().unwrap_or("<unnamed>")
            ))
        })?;
    Ok(UsageWindow {
        utilization_pct: pct_used(remaining),
        resets_at: parse_ts(bucket["resetTime"].as_str()),
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
    let used = (1.0 - remaining_fraction.clamp(0.0, 1.0)) * 100.0;
    used.round() as i32
}

fn parse_ts(s: Option<&str>) -> Option<DateTime<Utc>> {
    s.and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
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
    if let Some(addr) = override_addr {
        let addr = addr.trim();
        if !addr.is_empty() {
            return vec![normalize_base(addr)];
        }
    }

    // No hardcoded fallback port on purpose: the server always binds with
    // `--https_server_port 0`, so its port is drawn from the ephemeral range
    // and cannot be guessed. Probing a fixed one would just poke whatever
    // unrelated process happens to own it. Discovery or the explicit override.
    discovered
        .into_iter()
        .map(|p| format!("http://127.0.0.1:{p}"))
        .collect()
}

fn normalize_base(addr: &str) -> String {
    if addr.starts_with("http://") || addr.starts_with("https://") {
        addr.to_string()
    } else {
        format!("http://{addr}")
    }
}

/// Does this process look like one of the three Antigravity products?
///
/// Antigravity 2.0 and the IDE spawn a separate `language_server` child, while
/// the `agy` CLI embeds the same CSRF/RPC surface in its own process — so
/// matching on the server binary alone would miss a CLI-only install. `comm` is
/// truncated to 15 bytes by the kernel, which `language_server` exactly fills.
fn is_antigravity_process(comm: &str, exe: Option<&str>) -> bool {
    let comm = comm.trim();
    if comm.contains("language_server") || comm == "agy" || comm == "antigravity" {
        return true;
    }
    exe.is_some_and(|p| p.contains("antigravity") || p.ends_with("/agy"))
}

/// Loopback ports listened on by any running Antigravity product.
///
/// Reads `/proc` directly rather than shelling out to `ss`/`lsof`: find the
/// candidate pids, collect their socket inodes, then keep the listening TCP
/// entries owning one of those inodes. All three products report the *same*
/// shared quota, so whichever answers first is authoritative.
#[cfg(target_os = "linux")]
fn discover_ls_ports() -> Vec<u16> {
    use std::collections::HashSet;

    let mut inodes: HashSet<u64> = HashSet::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let pid_dir = entry.path();
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
                inodes.insert(ino);
            }
        }
    }

    if inodes.is_empty() {
        return Vec::new();
    }

    let mut ports = Vec::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(contents) = std::fs::read_to_string(table) else {
            continue;
        };
        for line in contents.lines().skip(1) {
            if let Some((port, ino)) = parse_proc_net_line(line)
                && inodes.contains(&ino)
                && !ports.contains(&port)
            {
                ports.push(port);
            }
        }
    }
    ports
}

#[cfg(not(target_os = "linux"))]
fn discover_ls_ports() -> Vec<u16> {
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

fn fallback_silent(cache: &Cache) -> Result<FetchOutcome> {
    let Some(bytes) = cache.fallback_payload(MAX_STALE)? else {
        return Err(AppError::Transport(
            "antigravity: no cache and language server unreachable".into(),
        ));
    };
    reuse_cache(bytes, cache, true, None)
}

/// Serve the stale cache when there is one. With no cache to fall back on,
/// surface `reason` — the actual diagnosis, e.g. "no local language server
/// found" — rather than a generic cache-miss that tells the user nothing about
/// what to do. This is the first-run path: no cache yet and Antigravity closed.
fn fallback_with_error(
    cache: &Cache,
    last_error: Option<(u16, String)>,
    reason: AppError,
) -> Result<FetchOutcome> {
    let Some(bytes) = cache.fallback_payload(MAX_STALE)? else {
        return Err(reason);
    };
    let mut outcome = reuse_cache(bytes, cache, true, None)?;
    outcome.last_error = last_error;
    Ok(outcome)
}

fn reuse_cache(
    bytes: Vec<u8>,
    cache: &Cache,
    stale: bool,
    account: Option<&str>,
) -> Result<FetchOutcome> {
    let snap = parse_cache(&bytes, account)?;
    Ok(FetchOutcome {
        snapshot: snap,
        stale,
        last_error: cache.read_last_error(),
        cache_age: cache.payload_age(),
    })
}

/// `account` is the fingerprint of the currently signed-in account, or `None`
/// when no local server answered. A payload belonging to a different account is
/// rejected so a Google-account switch cannot show the previous account's
/// quota. With `None` we cannot verify — but nothing is consuming quota while
/// Antigravity is down, so the last known figures are the best available truth.
pub fn parse_cache(bytes: &[u8], account: Option<&str>) -> Result<AntigravitySnapshot> {
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
    let window = |pct_key: &'static str, reset_key: &str, weekly: bool| {
        let pct = v[pct_key].as_i64().ok_or_else(|| {
            AppError::Other(format!("antigravity: cached payload missing {pct_key}"))
        })?;
        Ok::<_, AppError>(UsageWindow {
            utilization_pct: pct as i32,
            resets_at: parse_ts(v[reset_key].as_str()),
            window_duration: if weekly {
                chrono::Duration::days(7)
            } else {
                chrono::Duration::hours(5)
            },
        })
    };

    let optional = |pct_key: &str, reset_key: &str, weekly: bool| {
        v[pct_key].as_i64().map(|pct| UsageWindow {
            utilization_pct: pct as i32,
            resets_at: parse_ts(v[reset_key].as_str()),
            window_duration: if weekly {
                chrono::Duration::days(7)
            } else {
                chrono::Duration::hours(5)
            },
        })
    };

    Ok(AntigravitySnapshot {
        plan: v["plan"].as_str().unwrap_or(DEFAULT_PLAN).to_string(),
        account: cached_account.unwrap_or_default().to_string(),
        session: window("session_pct", "session_reset", false)?,
        weekly: window("weekly_pct", "weekly_reset", true)?,
        third_party_session: optional("tp_session_pct", "tp_session_reset", false),
        third_party_weekly: optional("tp_weekly_pct", "tp_weekly_reset", true),
    })
}

pub fn snap_to_json(snap: &AntigravitySnapshot) -> serde_json::Value {
    serde_json::json!({
        "plan": snap.plan,
        "account": snap.account,
        "session_pct": snap.session.utilization_pct,
        "session_reset": snap.session.resets_at.map(|dt| dt.to_rfc3339()),
        "weekly_pct": snap.weekly.utilization_pct,
        "weekly_reset": snap.weekly.resets_at.map(|dt| dt.to_rfc3339()),
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

    fn parsed() -> AntigravitySnapshot {
        let v: serde_json::Value = serde_json::from_str(QUOTA_JSON).unwrap();
        parse_quota_summary(&v, "Google AI Pro".into()).unwrap()
    }

    #[test]
    fn quota_summary_maps_four_distinct_windows() {
        let snap = parsed();
        assert_eq!(snap.plan, "Google AI Pro");
        // remainingFraction is inverted into "used".
        assert_eq!(snap.session.utilization_pct, 43);
        assert_eq!(snap.weekly.utilization_pct, 8);
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
        assert_eq!(snap.session.resets_at, at("2026-07-22T17:47:00Z"));
        assert_eq!(snap.weekly.resets_at, at("2026-07-28T17:39:58Z"));
        assert_eq!(
            snap.third_party_weekly.as_ref().unwrap().resets_at,
            at("2026-07-29T12:47:00Z")
        );
        // Regression: weekly must never be a copy of the 5h window.
        assert_ne!(snap.session.resets_at, snap.weekly.resets_at);
    }

    #[test]
    fn window_durations_match_their_bucket() {
        let snap = parsed();
        assert_eq!(snap.session.window_duration, chrono::Duration::hours(5));
        assert_eq!(snap.weekly.window_duration, chrono::Duration::days(7));
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
        assert_eq!(snap.session.utilization_pct, 50);
        assert_eq!(snap.weekly.utilization_pct, 10);
        assert_eq!(snap.third_party_session.unwrap().utilization_pct, 100);
        assert!(snap.third_party_weekly.is_none());
    }

    /// A drifted bucket must fail the parse rather than report a reassuring
    /// "0% used" for a window whose real state is unknown.
    #[test]
    fn a_bucket_without_a_usable_fraction_is_rejected() {
        for bad in [r#""oops""#, "null"] {
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
    fn missing_gemini_buckets_is_an_error_not_a_zero_bar() {
        let v: serde_json::Value = serde_json::from_str(r#"{"response":{"groups":[]}}"#).unwrap();
        assert!(parse_quota_summary(&v, "Pro".into()).is_err());
    }

    #[test]
    fn cache_round_trip_preserves_every_window() {
        let snap = parsed();
        let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap();
        assert_eq!(parse_cache(&bytes, None).unwrap(), snap);
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
            let err = parse_cache(&bytes, None).unwrap_err();
            assert!(err.to_string().contains(missing), "{missing}: {err}");
        }
        // A wholly empty object is not a zero-usage snapshot either.
        assert!(parse_cache(b"{}", None).is_err());
    }

    /// Switching Google accounts must not show the previous account's quota.
    #[test]
    fn a_cache_from_another_account_is_rejected() {
        let mut snap = parsed();
        snap.account = "acct:aaaa".into();
        let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap();

        assert!(parse_cache(&bytes, Some("acct:bbbb")).is_err());
        assert_eq!(parse_cache(&bytes, Some("acct:aaaa")).unwrap(), snap);

        // A payload written before the account was recorded is unattributable.
        let mut legacy = snap_to_json(&snap);
        legacy.as_object_mut().unwrap().remove("account");
        let legacy = serde_json::to_vec(&legacy).unwrap();
        assert!(parse_cache(&legacy, Some("acct:aaaa")).is_err());
    }

    /// With no local server there is nothing to compare against — and nothing
    /// is consuming quota either, so the last known figures still stand.
    #[test]
    fn an_unverifiable_cache_is_served_rather_than_discarded() {
        let mut snap = parsed();
        snap.account = "acct:aaaa".into();
        let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap();
        assert_eq!(parse_cache(&bytes, None).unwrap(), snap);
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
        assert_eq!(parse_cache(&bytes, None).unwrap(), snap);
    }

    #[test]
    fn cache_round_trip_preserves_absent_third_party_windows() {
        let mut snap = parsed();
        snap.third_party_session = None;
        snap.third_party_weekly = None;
        let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap();
        assert_eq!(parse_cache(&bytes, None).unwrap(), snap);
    }

    #[test]
    fn pct_used_inverts_and_clamps() {
        assert_eq!(pct_used(1.0), 0);
        assert_eq!(pct_used(0.0), 100);
        assert_eq!(pct_used(0.5), 50);
        // Guard against a server sending a fraction outside [0,1].
        assert_eq!(pct_used(1.5), 0);
        assert_eq!(pct_used(-0.5), 100);
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
    fn explicit_address_wins_and_gets_a_scheme() {
        assert_eq!(
            candidate_bases_with(Some("127.0.0.1:1234"), vec![5678]),
            vec!["http://127.0.0.1:1234".to_string()]
        );
        // An address that already carries a scheme is left alone.
        assert_eq!(
            candidate_bases_with(Some("https://host:9"), vec![]),
            vec!["https://host:9".to_string()]
        );
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
    }

    #[test]
    fn unrelated_processes_are_not_probed() {
        assert!(!is_antigravity_process("sshd", Some("/usr/sbin/sshd")));
        assert!(!is_antigravity_process("node", Some("/usr/bin/node")));
        // "legacy" ends in a substring of "/agy" but is not the CLI.
        assert!(!is_antigravity_process("legacy", Some("/usr/bin/legacy")));
        assert!(!is_antigravity_process("", None));
    }

    /// First run with Antigravity closed: no cache to serve, so the user must
    /// be told what to start — not "no usable cache", which says nothing.
    #[test]
    fn missing_cache_surfaces_the_diagnosis_not_a_cache_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().join("usage.json"));
        let reason = AppError::Credentials("Antigravity: no local language server found".into());

        let err = fallback_with_error(&cache, None, reason).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no local language server found"), "{msg}");
        assert!(!msg.contains("no usable cache"), "{msg}");
    }

    #[test]
    fn blank_override_falls_through_to_discovery() {
        assert_eq!(
            candidate_bases_with(Some("   "), vec![4242]),
            vec!["http://127.0.0.1:4242".to_string()]
        );
    }
}
