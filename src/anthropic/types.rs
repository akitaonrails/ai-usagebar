//! Wire types for the Anthropic OAuth usage endpoint.
//!
//! Every field is `Option<T>` or has `#[serde(default)]` — the endpoint is
//! undocumented and the shape varies across plan tiers and over time. The
//! lossy `serde(default)` approach matches claudebar's jq pattern of
//! `.field // empty`.

use serde::{Deserialize, Serialize};

use super::creds::ULTRA;
use crate::usage::{
    AnthropicSnapshot, Cents, CloudCredits, ExtraUsage, ExtraUsageStatus, ResetCredit,
    ResetCredits, ScopedWindow, UsageCredits, UsageWindow, checked_reset_title,
};

/// Top-level response from `GET /api/oauth/usage`.
///
/// No `PartialEq`: `iguana_necktie` and `spend` are raw JSON (`RawValue`
/// has no equality), and the snapshot — the value frontends compare —
/// carries the equality that matters.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct UsageResponse {
    #[serde(default)]
    pub five_hour: Option<Window>,
    #[serde(default)]
    pub seven_day: Option<Window>,
    #[serde(default)]
    pub seven_day_sonnet: Option<Window>,
    #[serde(default)]
    pub extra_usage: Option<ExtraUsageBlock>,
    /// Newer per-limit array. Carries model-scoped weekly windows
    /// (`kind == "weekly_scoped"`, e.g. the Fable weekly cap) that have no
    /// dedicated `seven_day_*` field.
    #[serde(default)]
    pub limits: Vec<LimitEntry>,
    /// Banked limit resets. Present only when the request asks for them
    /// (`?cedar_ember=1`) *and* the endpoint accepts the caller's surface —
    /// otherwise the key is absent or null, which is not an error: most
    /// accounts have no grant most of the time.
    #[serde(default)]
    pub cedar_ember: Option<ResetGrantsBlock>,
    /// Cloud session credits (the money Claude Code's cloud sessions draw
    /// from). Absent for plans without the grant. Captured as raw JSON and
    /// interpreted after the fact, so a value the parser itself cannot
    /// materialize (a `1e400` overflows every number type) degrades to
    /// `None` for the block alone — like every block here, drift never
    /// fails the response.
    #[serde(default)]
    pub iguana_necktie: Option<Box<serde_json::value::RawValue>>,
    /// The prepaid usage-credit state. Captured raw for the same reason
    /// `iguana_necktie` is: the filled-in shape of `balance` has never been
    /// observed and is interpreted after the fact, and a number the parser
    /// cannot materialize must cost the block, not the response.
    #[serde(default)]
    pub spend: Option<Box<serde_json::value::RawValue>>,
}

/// The `cedar_ember` block. Every field is optional because the endpoint
/// answers with a partial block for an ineligible caller (`eligible: false`,
/// `ineligible_reason: "surface" | "cli_version" | …`) and the reason is not
/// ours to render — a status bar has nothing to say about an offer the
/// account cannot take.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct ResetGrantsBlock {
    #[serde(default)]
    pub eligible: bool,
    #[serde(default)]
    pub grants: Vec<ResetGrant>,
}

/// One banked grant. `id` is the handle that *spends* the reset, so — as with
/// Codex's `credits[].id` and SuperGrok's `token_id` — it is never
/// deserialized, and nothing downstream can leak what was never held.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct ResetGrant {
    /// e.g. "Claude Opus 5.5 launch: one usage-limit reset for Team members".
    #[serde(default)]
    pub label: Option<String>,
    /// How many of this grant's resets are still unspent. A grant that has
    /// been fully redeemed stays in the array at 0.
    #[serde(default)]
    pub resets_left: u32,
    /// RFC3339. The deadline the user is actually racing.
    #[serde(default)]
    pub ends_at: Option<String>,
    /// The server's own verdict on whether the reset can be redeemed right
    /// now — it accounts for `starts_at`, cooldowns, and campaign state.
    /// Defaulting to false matches the official client and keeps a grant we
    /// cannot vouch for off the bar.
    #[serde(default)]
    pub usable_now: bool,
    #[serde(default)]
    pub paused: bool,
}

/// One entry of the `limits[]` array. Only `weekly_scoped` entries with a
/// model display name are lifted into the snapshot; everything else in the
/// array duplicates `five_hour`/`seven_day`.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct LimitEntry {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "de_percent_opt")]
    pub percent: Option<f64>,
    #[serde(default)]
    pub resets_at: Option<String>,
    #[serde(default)]
    pub scope: Option<LimitScope>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct LimitScope {
    #[serde(default)]
    pub model: Option<LimitModel>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct LimitModel {
    #[serde(default)]
    pub display_name: Option<String>,
}

/// A single usage window — `utilization` is `0..=100` (integer percent).
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct Window {
    #[serde(default, deserialize_with = "de_percent")]
    pub utilization: f64,
    #[serde(default)]
    pub resets_at: Option<String>,
}

/// Pay-as-you-go extra usage. Both money values are non-negative integer minor
/// units, but the API sometimes returns integral floats (e.g. `0.0`). Missing
/// values remain absent so an enabled but incomplete block cannot manufacture
/// a zero balance.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct ExtraUsageBlock {
    #[serde(default)]
    pub is_enabled: bool,
    #[serde(default, deserialize_with = "de_opt_cents")]
    pub monthly_limit: Option<i64>,
    #[serde(default, deserialize_with = "de_opt_cents")]
    pub used_credits: Option<i64>,
    /// ISO currency code (`"BRL"`, `"USD"`, …). Absent on older payloads,
    /// which were always formatted as `$`.
    #[serde(default, deserialize_with = "de_opt_currency")]
    pub currency: Option<String>,
    /// Minor-unit digits for both money fields. Gated at the parse boundary:
    /// an absurd scale would corrupt every formatted amount downstream.
    #[serde(default, deserialize_with = "de_opt_decimal_places")]
    pub decimal_places: Option<u32>,
    #[serde(default)]
    pub user_disabled: bool,
    #[serde(default)]
    pub spend_limit_reached: bool,
    #[serde(default)]
    pub credits_ever_enabled: bool,
    #[serde(default)]
    pub disabled_reason: Option<String>,
}

/// The `iguana_necktie` block: the cloud session credits Claude Code's
/// cloud sessions draw from. Every field is `Option` — the block arrives
/// incomplete across plan tiers and over time — and every field deserializes
/// leniently, so a field of an unexpected type becomes `None` for that field
/// alone instead of failing the whole response. `utilization` and
/// `used_dollars` are not lifted into the snapshot and are not read here.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct CloudCreditsBlock {
    /// Parsed as the raw JSON number, not as an `f64`: the conversion to
    /// cents is exact integer arithmetic (`dollars_to_cents`), and a number
    /// the parser cannot materialize at all fails this re-parse, degrading
    /// the block rather than the response.
    #[serde(default, deserialize_with = "de_opt_number")]
    pub limit_dollars: Option<serde_json::Number>,
    #[serde(default, deserialize_with = "de_opt_number")]
    pub remaining_dollars: Option<serde_json::Number>,
    /// RFC 3339 on the wire; the snapshot keeps `None` when it does not parse.
    #[serde(default, deserialize_with = "de_opt_plain_string")]
    pub resets_at: Option<String>,
    /// A non-empty string is a lock; the text itself is not rendered,
    /// so it is never lifted out of the parser.
    #[serde(default, deserialize_with = "de_opt_plain_string")]
    pub locked_reason: Option<String>,
}

/// The `spend` block, read raw. The endpoint's filled-in `balance` has never
/// been observed, so the two fields that are read stay JSON and are
/// interpreted after the fact; the rest of the block is not read in this
/// slice.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct SpendBlock {
    #[serde(default)]
    pub balance: Option<serde_json::Value>,
    #[serde(default)]
    pub auto_reload: Option<serde_json::Value>,
}

/// Re-parse a raw-captured block into its typed form, in two passes.
///
/// The first pass is to `Value` — this is where a number the parser cannot
/// materialize (`1e400`) fails, costing only the block. The second accepts
/// only an object, because an all-`#[serde(default)]` struct also
/// deserializes from a *sequence*, filling its fields positionally: without
/// the gate, `[1, 2]` would read as `limit_dollars: 1, remaining_dollars: 2`,
/// the fabrication the drift discipline exists to prevent.
fn raw_object_block<T>(raw: &serde_json::value::RawValue) -> Option<T>
where
    T: serde::de::DeserializeOwned,
{
    let v = serde_json::from_str::<serde_json::Value>(raw.get()).ok()?;
    match v {
        serde_json::Value::Object(_) => serde_json::from_value::<T>(v).ok(),
        _ => None,
    }
}

/// A raw JSON number that may be absent, null, or the wrong type: each
/// degradation is `None` for that field alone. The number is handed on
/// unconverted — the finite/range discipline lives in `dollars_to_cents`,
/// in exact arithmetic, where a float round-trip could not prove it.
fn de_opt_number<'de, D>(d: D) -> std::result::Result<Option<serde_json::Number>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(n) => Ok(Some(n)),
        _ => Ok(None),
    }
}

/// Dollars on the wire → whole cents in exact arithmetic. One function for
/// `limit_dollars` and `remaining_dollars` so the two cannot drift.
///
/// The size gate runs on the *value*, not the notation: the parser's own
/// text for `1e14` carries no `e` (it prints `1000000000000000`), so a
/// notation-only check would let a $100-trillion "limit" through as a
/// confident figure. Past 1e13 dollars the number is beyond any grant this
/// endpoint states, and 1e13 keeps `* 100` far inside i64
/// (1e15 << 9.2e18), so the scientific-notation branch cannot overflow.
///
/// The plain-decimal path splits integer and fractional parts and works in
/// integer arithmetic: `(f64 * 100.0).round()` loses monetary ties, because
/// the product is binary (1.005 dollars is 100.5 binary cents, which rounds
/// to 100 instead of the 101 the money says). The first two fractional
/// digits are exact; the third decides the tie, half rounding up.
///
/// Negative, non-finite, or past the gate: `None`. A missing figure is never
/// a fabricated one.
fn dollars_to_cents(n: &serde_json::Number) -> Option<i64> {
    let v = n.as_f64()?;
    if !v.is_finite() || v < 0.0 || v > 1e13 {
        return None;
    }
    // -0 passes the sign check but prints as "-0.0", which the digit walk
    // below would reject: an exhausted grant reported as -0 is still $0.00.
    if v == 0.0 {
        return Some(0);
    }
    let text = n.to_string();
    if text.contains(['e', 'E']) {
        return Some((v * 100.0).round() as i64);
    }
    let (int_part, frac_part) = match text.split_once('.') {
        Some((i, f)) => (i, f),
        None => (text.as_str(), ""),
    };
    if int_part.is_empty() || int_part.bytes().any(|b| !b.is_ascii_digit()) {
        return None;
    }
    let mut cents: i64 = 0;
    for b in int_part.bytes() {
        cents = cents.checked_mul(10)?.checked_add(i64::from(b - b'0'))?;
    }
    cents = cents.checked_mul(100)?;
    let digits: Vec<u8> = frac_part.bytes().filter(|b| b.is_ascii_digit()).collect();
    let first = i64::from(digits.first().copied().unwrap_or(b'0') - b'0');
    let second = i64::from(digits.get(1).copied().unwrap_or(b'0') - b'0');
    let half_up = i64::from(digits.get(2).is_some_and(|d| *d >= b'5'));
    cents.checked_add(first * 10 + second + half_up)
}

/// A string field that may be absent, null, or the wrong type: each
/// degradation is `None` for that field alone.
fn de_opt_plain_string<'de, D>(d: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => Ok(Some(s)),
        _ => Ok(None),
    }
}

/// Accept a plausible minor-unit scale (0..=6 covers every ISO 4217 currency;
/// the largest real exponent is 4). Integral floats are tolerated for the same
/// reason `de_opt_cents` tolerates them: this endpoint emits them (the #30
/// payload carries `used_credits: 14157.0`), and rejecting `2.0` would fail
/// the whole response over a value that is unambiguous. Null/absent remains
/// absent: a currency code alone is not enough to infer every ISO exponent.
/// Anything else is drift: a wire `decimal_places: 100` would overflow the
/// scale and mis-state every amount, so it fails loudly as `⚠` instead.
fn de_opt_decimal_places<'de, D>(d: D) -> std::result::Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let n = match serde_json::Value::deserialize(d)? {
        serde_json::Value::Null => return Ok(None),
        serde_json::Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)),
        _ => None,
    };
    match n {
        Some(n) if (0..=6).contains(&n) => Ok(Some(n as u32)),
        _ => Err(serde::de::Error::custom(
            "decimal_places must be an integer in 0..=6",
        )),
    }
}

/// Gate the currency to a plausible ISO 4217 alpha code. The value is embedded
/// verbatim in Pango bar markup and in the `;;`-delimited desktop FORMAT
/// protocol, so an arbitrary string is an injection vector as well as drift;
/// three ASCII uppercase letters can be neither.
fn de_opt_currency<'de, D>(d: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<String>::deserialize(d)? {
        None => Ok(None),
        Some(s) if s.len() == 3 && s.bytes().all(|b| b.is_ascii_uppercase()) => Ok(Some(s)),
        Some(s) => Err(serde::de::Error::custom(format!(
            "currency {s:?} is not an ISO 4217 alpha code"
        ))),
    }
}

fn de_opt_cents<'de, D>(d: D) -> std::result::Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                if i >= 0 {
                    Ok(Some(i))
                } else {
                    Err(serde::de::Error::custom("cents cannot be negative"))
                }
            } else if let Some(f) = n.as_f64() {
                const MAX_EXACT_F64_INT: f64 = (1_u64 << 53) as f64;
                if f.is_finite() && f.fract() == 0.0 && (0.0..=MAX_EXACT_F64_INT).contains(&f) {
                    Ok(Some(f as i64))
                } else {
                    Err(serde::de::Error::custom(
                        "cents must be a non-negative integer in range",
                    ))
                }
            } else {
                Err(serde::de::Error::custom("number out of i64 range"))
            }
        }
        other => Err(serde::de::Error::custom(format!(
            "expected number or null, got {other:?}"
        ))),
    }
}

/// Slack tolerated above 100. The endpoint occasionally reports a hair over its
/// own cap — `used/limit` rounding, or usage that landed just before the block
/// did — and `to_window` saturates that back to 100.
const PCT_SLACK: f64 = 1.0;

/// Gate a wire percentage into `0..=100` (+ [`PCT_SLACK`]).
///
/// Rejecting here rather than in `to_window` is deliberate: `to_window` is
/// infallible and `into_snapshot` has no error channel, so the parse boundary
/// is the only place a bad value can still become a loud failure. A rejection
/// surfaces as `AppError::Json` and reaches the user as `⚠` — never as a
/// number we invented. Past the slack the field simply isn't a percentage on
/// this scale (rescaled to per-mille, a raw counter, a sentinel), and clamping
/// it would paint a "100%" bar we cannot vouch for. Non-finite values matter
/// most: `f64::NAN as i32` is silently `0`.
fn checked_percent<E: serde::de::Error>(v: f64) -> std::result::Result<f64, E> {
    if !v.is_finite() {
        return Err(E::custom(format!("percentage {v} is not finite")));
    }
    if !(0.0..=100.0 + PCT_SLACK).contains(&v) {
        return Err(E::custom(format!("percentage {v} outside 0..=100")));
    }
    Ok(v)
}

fn de_percent<'de, D>(d: D) -> std::result::Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    checked_percent(f64::deserialize(d)?)
}

fn de_percent_opt<'de, D>(d: D) -> std::result::Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<f64>::deserialize(d)?
        .map(checked_percent)
        .transpose()
}

impl UsageResponse {
    /// Lift the wire response into our canonical [`AnthropicSnapshot`].
    ///
    /// `plan_label` is the rendered plan name ("Max", "Ultra" etc.), derived
    /// from the credentials file (since the usage endpoint doesn't include it).
    ///
    /// The plan also decides what a missing `five_hour` means. Verified Ultra
    /// accounts have no five-hour limit, and the endpoint reports that as
    /// `five_hour: null`; claude.ai's own check is the same ("If you don't
    /// see a Current session bar, your account is verified"), so the snapshot
    /// carries no session window. An unverified Ultra account has a temporary
    /// five-hour limit and still reports the window. Every other plan has the
    /// limit, so a missing window there still reads as an idle one at 0%.
    pub fn into_snapshot(self, plan_label: String) -> AnthropicSnapshot {
        // Window durations are constants per claudebar:172-173.
        const SESSION: chrono::Duration = chrono::Duration::hours(5);
        const WEEKLY: chrono::Duration = chrono::Duration::days(7);

        fn to_window(w: Option<Window>, dur: chrono::Duration) -> UsageWindow {
            let Some(w) = w else {
                return UsageWindow {
                    utilization_pct: 0,
                    resets_at: None,
                    window_duration: dur,
                };
            };
            UsageWindow {
                // Round to nearest, matching claudebar's `| round` jq filter,
                // then absorb the overshoot `de_percent` deliberately lets
                // through (100.4 → 100) so the bar never renders past full.
                utilization_pct: i32::from(crate::format::clamp_pct(w.utilization)),
                resets_at: w
                    .resets_at
                    .as_deref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc)),
                window_duration: dur,
            }
        }

        let session = match self.five_hour {
            None if plan_label == ULTRA => None,
            five_hour => Some(to_window(five_hour, SESSION)),
        };
        let weekly = to_window(self.seven_day, WEEKLY);
        let sonnet = self.seven_day_sonnet.map(|w| to_window(Some(w), WEEKLY));
        let extra = self.extra_usage.and_then(|e| {
            let status = if e.is_enabled {
                ExtraUsageStatus::Active
            } else if e.user_disabled {
                return None;
            } else {
                match e.disabled_reason.as_deref() {
                    Some("out_of_credits") => ExtraUsageStatus::OutOfCredits,
                    Some("monthly_limit_reached" | "spend_limit_reached") => {
                        ExtraUsageStatus::MonthlyLimitReached
                    }
                    _ if e.spend_limit_reached => ExtraUsageStatus::MonthlyLimitReached,
                    _ if e.credits_ever_enabled && e.used_credits.unwrap_or_default() > 0 => {
                        ExtraUsageStatus::Unavailable
                    }
                    _ => return None,
                }
            };
            Some(ExtraUsage {
                // `monthly_limit: null` is semantic, not drift: the endpoint
                // sends it for plans with no spending cap (e.g. Pro), so it
                // maps to None instead of discarding the block — which hid
                // real credit spend (#30). Only an unusable `used_credits`
                // still drops it: without the spend there is nothing to show.
                limit: e.monthly_limit.map(Cents),
                spent: Cents(e.used_credits?),
                // Preserve an absent scale. Currency codes span zero through
                // four decimal minor units, so guessing would fabricate the
                // displayed major-unit amount.
                decimal_places: e.decimal_places,
                currency: e.currency,
                status,
            })
        });
        let scoped = self
            .limits
            .into_iter()
            .filter(|l| l.kind.as_deref() == Some("weekly_scoped"))
            .filter_map(|l| {
                let label = l.scope?.model?.display_name?;
                // Same `?` discipline as the label above: an entry without a
                // percentage is dropped, not defaulted. `unwrap_or(0.0)` drew a
                // confident "Fable 0%" bar under a real model name — a number
                // the API never sent, which is worse than no bar at all.
                let utilization = l.percent?;
                let window = to_window(
                    Some(Window {
                        utilization,
                        resets_at: l.resets_at,
                    }),
                    WEEKLY,
                );
                Some(ScopedWindow { label, window })
            })
            .collect();

        let cloud_credits = self
            .iguana_necktie
            .as_ref()
            .and_then(|raw| raw_object_block::<CloudCreditsBlock>(raw))
            .and_then(cloud_credits_from);
        let usage_credits = self
            .spend
            .as_ref()
            .and_then(|raw| raw_object_block::<SpendBlock>(raw))
            .and_then(usage_credits_from);
        AnthropicSnapshot {
            plan: plan_label,
            session,
            weekly,
            sonnet,
            scoped,
            extra,
            reset_credits: reset_credits(self.cedar_ember),
            cloud_credits,
            usage_credits,
        }
    }
}

/// Lift the `iguana_necktie` block into snapshot credits.
///
/// Only a finite, positive limit paired with a finite, non-negative
/// remaining is a block we can state: with no limit there is no denominator
/// for "left of", and a negative remaining is drift, not a balance. `used`
/// is deliberately not kept — the panel meters the remaining against the
/// limit, and storing a third money field the renderers would never read is
/// where copies drift.
fn cloud_credits_from(block: CloudCreditsBlock) -> Option<CloudCredits> {
    // Dollars → whole cents at the boundary: the snapshot derives `Eq`, so
    // the money lives as integer cents, not the float the wire sent.
    let limit = dollars_to_cents(block.limit_dollars.as_ref()?)?;
    let remaining = dollars_to_cents(block.remaining_dollars.as_ref()?)?;
    // The gate runs on the *converted* cents: a positive dollar limit that
    // the conversion rounds to zero (e.g. 0.004) would divide by zero
    // downstream and report a confident 0% meter for a fully spent grant.
    if limit <= 0 || remaining < 0 {
        return None;
    }
    Some(CloudCredits {
        limit: Cents(limit),
        remaining: Cents(remaining),
        expires_at: block
            .resets_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc)),
        // A non-empty reason is the lock. The text itself is untrusted and
        // never rendered, so only the flag crosses this boundary.
        locked: block.locked_reason.is_some_and(|r| !r.is_empty()),
    })
}

/// A JSON number in the sense the balance fields use it: an integer, or an
/// integral float — this endpoint floats its numbers (`used_credits: 14157.0`
/// is a real #30 wire value), and rejecting `1500.0` would fail over a
/// value that is unambiguous.
fn json_int(v: &serde_json::Value) -> Option<i64> {
    let n = v.as_number()?;
    // The integer is the value it claims to be. A `u64` count only passes
    // when it also fits `i64` — `u64::MAX` converted to cents would
    // silently wrap to a negative balance.
    if let Some(i) = n.as_i64() {
        return Some(i);
    }
    if let Some(u) = n.as_u64() {
        return i64::try_from(u).ok();
    }
    // This endpoint floats its numbers (`used_credits: 14157.0` is a real
    // #30 wire value), so an integral float is the same count — but only
    // while `f64` can still represent every integer it holds (|v| ≤ 2^53);
    // beyond that the "integer" has already lost digits.
    n.as_f64()
        .filter(|f| f.is_finite() && f.fract() == 0.0 && f.abs() <= 2_f64.powi(53))
        .map(|f| f as i64)
}

/// Interpret the raw `spend` block into [`UsageCredits`].
///
/// The filled-in shape of `spend.balance` has never been observed on the
/// wire, so the parser hands it over as JSON and this decides, after the
/// fact, which of the two plausible shapes it is. Anything else is `None`:
/// a balance the response does not report in a shape we can express is not
/// shown — never as `$0.00`.
fn usage_credits_from(spend: SpendBlock) -> Option<UsageCredits> {
    let balance_value = spend.balance?;
    let balance = balance_value.as_object()?;
    let currency = balance
        .get("currency")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // Shape (a): `{"amount_minor", "currency", "exponent"}` — minor units
    // with their own scale. When all three fields are usable it wins over
    // (b): it carries the exact scale that (b) could only guess. An object
    // missing one of the three is not shape (a) — it may still be shape (b),
    // which is tested next. (b) is `{"amount", "currency"}` with `amount` in
    // minor units, whose scale is only knowable for USD (two minor digits);
    // another currency in that shape is dropped, not rescaled.
    let shaped = (|| {
        let minor = balance.get("amount_minor").and_then(json_int)?;
        let exponent = balance
            .get("exponent")
            .and_then(json_int)
            .filter(|e| (0..=4).contains(e))?;
        let currency = currency.as_ref()?;
        Some((minor, exponent as u32, currency.clone()))
    })()
    .or_else(|| {
        let amount = balance.get("amount").and_then(json_int)?;
        let currency = currency.as_ref().filter(|c| *c == "USD")?;
        Some((amount, 2, currency.clone()))
    });
    let (amount, decimal_places, currency) = shaped?;

    // `auto_reload`: each candidate is a boolean or it is not a signal at
    // all. The top-level `spend.auto_reload.enabled` wins when it is one;
    // a non-boolean there (the richer shape reports a word) falls through
    // to the balance's own `auto_reload_settings.enabled` when that is a
    // boolean; otherwise not reported — `None`, never a guessed "off".
    let auto_reload = spend
        .auto_reload
        .as_ref()
        .and_then(|v| v.get("enabled"))
        .and_then(|v| v.as_bool())
        .or_else(|| {
            balance
                .get("auto_reload_settings")
                .and_then(|v| v.get("enabled"))
                .and_then(|v| v.as_bool())
        });

    Some(UsageCredits {
        balance: Cents(amount),
        decimal_places,
        currency: Some(currency),
        auto_reload,
    })
}

/// Project the `cedar_ember` block onto the shared banked-reset model.
///
/// Only grants the server says are redeemable are counted: `eligible` gates
/// the whole block, and a grant must be unspent, not paused, and `usable_now`.
/// A grant that has not opened yet (`starts_at` in the future) reports
/// `usable_now: false`, and counting it would put a reset on the bar that the
/// account cannot actually use.
///
/// Expiry is left to the renderer rather than filtered here: `into_snapshot`
/// has no clock, and [`crate::format::reset_credit_lines`] already renders a
/// lapsed credit as "expired <date>" — truthful either way, and the server
/// retires the grant on its own schedule.
fn reset_credits(block: Option<ResetGrantsBlock>) -> ResetCredits {
    let Some(block) = block.filter(|block| block.eligible) else {
        return ResetCredits::default();
    };
    let usable = block
        .grants
        .into_iter()
        .filter(|grant| grant.usable_now && !grant.paused && grant.resets_left > 0);
    let mut credits = ResetCredits::default();
    for grant in usable {
        credits.available = credits.available.saturating_add(grant.resets_left);
        credits.credits.push(ResetCredit {
            title: checked_reset_title(grant.label),
            expires_at: grant
                .ends_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc)),
        });
    }
    credits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_response() {
        let raw = r#"{
            "five_hour":         {"utilization": 42.7, "resets_at": "2026-05-23T17:30:00Z"},
            "seven_day":         {"utilization": 27.0, "resets_at": "2026-05-30T12:00:00Z"},
            "seven_day_sonnet":  {"utilization":  4.2, "resets_at": "2026-05-30T12:00:00Z"},
            "extra_usage":       {"is_enabled": true, "monthly_limit": 5000, "used_credits": 250}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Max 5x".into());
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 43); // rounded
        assert_eq!(snap.weekly.utilization_pct, 27);
        assert_eq!(snap.sonnet.as_ref().unwrap().utilization_pct, 4);
        let extra = snap.extra.as_ref().unwrap();
        assert_eq!(extra.limit, Some(Cents(5000)));
        assert_eq!(extra.spent.0, 250);
        assert!(snap.session.as_ref().unwrap().resets_at.is_some());
    }

    #[test]
    fn parses_weekly_scoped_limits() {
        // Real shape observed 2026-07-08: the Fable weekly cap only exists
        // inside `limits[]`; there is no `seven_day_fable` field.
        let raw = r#"{
            "five_hour": {"utilization": 10.0, "resets_at": "2026-07-08T22:59:59Z"},
            "seven_day": {"utilization": 55.0, "resets_at": "2026-07-10T10:59:59Z"},
            "limits": [
                {"kind": "session", "group": "session", "percent": 10,
                 "severity": "normal", "resets_at": "2026-07-08T22:59:59Z",
                 "scope": null, "is_active": false},
                {"kind": "weekly_all", "group": "weekly", "percent": 55,
                 "severity": "normal", "resets_at": "2026-07-10T10:59:59Z",
                 "scope": null, "is_active": false},
                {"kind": "weekly_scoped", "group": "weekly", "percent": 84,
                 "severity": "warning", "resets_at": "2026-07-10T10:59:59Z",
                 "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null},
                 "is_active": true}
            ]
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert_eq!(snap.scoped.len(), 1);
        assert_eq!(snap.scoped[0].label, "Fable");
        assert_eq!(snap.scoped[0].window.utilization_pct, 84);
        assert!(snap.scoped[0].window.resets_at.is_some());
        // Unscoped entries never duplicate into `scoped`.
        assert_eq!(snap.weekly.utilization_pct, 55);
    }

    /// The shape a verified Ultra account returns (observed 2026-11-02, numbers
    /// and times changed): `five_hour` is null and `limits[]` has no session
    /// entry, because the account has no five-hour limit.
    const VERIFIED_ULTRA: &str = r#"{
        "five_hour": null,
        "seven_day": {"utilization": 41.0, "resets_at": "2026-11-06T18:00:00.123456+00:00",
                      "limit_dollars": null, "used_dollars": null,
                      "remaining_dollars": null, "locked_reason": null},
        "seven_day_sonnet": null,
        "limits": [
            {"kind": "weekly_all", "group": "weekly", "percent": 41,
             "severity": "normal", "resets_at": "2026-11-06T18:00:00.123456+00:00",
             "scope": null, "is_active": true},
            {"kind": "weekly_scoped", "group": "weekly", "percent": 33,
             "severity": "normal", "resets_at": "2026-11-06T18:00:00.123456+00:00",
             "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null},
             "is_active": false}
        ]
    }"#;

    #[test]
    fn a_verified_ultra_account_has_no_session_window() {
        let resp: UsageResponse = serde_json::from_str(VERIFIED_ULTRA).unwrap();
        let snap = resp.into_snapshot(ULTRA.into());
        assert_eq!(snap.session, None);
        assert_eq!(snap.weekly.utilization_pct, 41);
        assert!(snap.weekly.resets_at.is_some());
        assert_eq!(snap.scoped.len(), 1);
        assert_eq!(snap.scoped[0].label, "Fable");
        assert_eq!(snap.scoped[0].window.utilization_pct, 33);
    }

    /// Until it is verified, an Ultra account has a temporary five-hour limit
    /// and the endpoint reports it, so the window stays.
    #[test]
    fn an_unverified_ultra_account_keeps_its_five_hour_window() {
        let raw = r#"{
            "five_hour": {"utilization": 12.0, "resets_at": "2026-11-02T23:00:00Z"},
            "seven_day": {"utilization": 41.0, "resets_at": "2026-11-06T18:00:00Z"}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let session = resp.into_snapshot(ULTRA.into()).session.unwrap();
        assert_eq!(session.utilization_pct, 12);
        assert!(session.resets_at.is_some());
    }

    /// Ultra is the only plan with a documented state without the limit. On
    /// any other plan a missing window still reads as an idle one at 0%.
    #[test]
    fn other_plans_keep_the_idle_fallback_for_a_missing_five_hour_window() {
        for plan in ["Max", "Pro", "Team", "Unknown"] {
            let resp: UsageResponse = serde_json::from_str(VERIFIED_ULTRA).unwrap();
            let session = resp.into_snapshot(plan.into()).session;
            assert_eq!(
                session.as_ref().map(|s| s.utilization_pct),
                Some(0),
                "{plan}"
            );
            assert_eq!(session.and_then(|s| s.resets_at), None, "{plan}");
        }
    }

    #[test]
    fn missing_limits_array_yields_empty_scoped() {
        let raw = r#"{
            "five_hour": {"utilization": 0, "resets_at": "2026-05-23T17:30:00Z"},
            "seven_day": {"utilization": 0, "resets_at": "2026-05-30T12:00:00Z"}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert!(snap.scoped.is_empty());
    }

    #[test]
    fn missing_sonnet_and_extra_are_none() {
        let raw = r#"{
            "five_hour": {"utilization": 0, "resets_at": "2026-05-23T17:30:00Z"},
            "seven_day": {"utilization": 0, "resets_at": "2026-05-30T12:00:00Z"}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert!(snap.sonnet.is_none());
        assert!(snap.extra.is_none());
    }

    #[test]
    fn user_disabled_extra_usage_becomes_none() {
        let raw = r#"{
            "five_hour": {"utilization": 0},
            "seven_day": {"utilization": 0},
            "extra_usage": {"is_enabled": false, "user_disabled": true,
                            "monthly_limit": 5000, "used_credits": 0}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert!(snap.extra.is_none());
    }

    #[test]
    fn provider_disabled_extra_usage_keeps_spend_and_reason_visible() {
        let raw = r#"{
            "extra_usage": {
                "is_enabled": false,
                "monthly_limit": null,
                "used_credits": 1234.0,
                "currency": "USD",
                "decimal_places": 2,
                "disabled_reason": "out_of_credits",
                "user_disabled": false,
                "spend_limit_reached": false,
                "credits_ever_enabled": true
            }
        }"#;
        let extra = serde_json::from_str::<UsageResponse>(raw)
            .unwrap()
            .into_snapshot("Pro".into())
            .extra
            .unwrap();
        assert_eq!(extra.fmt_spent(), "$12.34");
        assert_eq!(extra.status, ExtraUsageStatus::OutOfCredits);
        assert_eq!(extra.status_note(), Some("out of credits"));
    }

    #[test]
    fn enabled_extra_usage_without_spend_is_dropped() {
        // `used_credits` is the essential datum: without it there is nothing
        // truthful to display, so the block is dropped rather than inventing
        // a $0.00 spend.
        for raw in [
            r#"{"extra_usage":{"is_enabled":true,"monthly_limit":5000}}"#,
            r#"{"extra_usage":{"is_enabled":true,"monthly_limit":5000,"used_credits":null}}"#,
        ] {
            let resp: UsageResponse = serde_json::from_str(raw).unwrap();
            assert!(resp.into_snapshot("Pro".into()).extra.is_none(), "{raw}");
        }
    }

    #[test]
    fn uncapped_plan_keeps_real_spend_visible() {
        // The #30 regression: the endpoint sends `monthly_limit: null` for
        // plans with no spending cap (e.g. Pro). Discarding the block hid
        // genuine credit spend — this fixture is the reporter's actual cached
        // response (R$ 141.57, an integral float).
        let resp: UsageResponse = serde_json::from_str(
            r#"{"extra_usage":{"is_enabled":true,"monthly_limit":null,
                "used_credits":14157.0,"currency":"BRL","decimal_places":2,
                "disabled_reason":null}}"#,
        )
        .unwrap();
        let extra = resp.into_snapshot("Pro".into()).extra.unwrap();
        assert_eq!(extra.limit, None);
        assert_eq!(extra.spent.0, 14157);
        // No denominator → no invented percentage; bar stays calm.
        assert_eq!(extra.percent(), 0);
        // The block's own currency and scale propagate, so the renderer can
        // say R$141.57 instead of claiming `$` for reais.
        assert_eq!(extra.currency.as_deref(), Some("BRL"));
        assert_eq!(extra.decimal_places, Some(2));
        assert_eq!(extra.fmt_spent(), "R$141.57");

        // An *absent* limit renders the same way: with `#[serde(default)]`,
        // absent and explicit null are indistinguishable at the struct level,
        // and hiding real spend because a secondary field went missing is the
        // exact failure mode of #30. Nothing is fabricated either way — the
        // spend shown is exactly what the API sent.
        let resp: UsageResponse =
            serde_json::from_str(r#"{"extra_usage":{"is_enabled":true,"used_credits":250}}"#)
                .unwrap();
        let extra = resp.into_snapshot("Pro".into()).extra.unwrap();
        assert_eq!(extra.limit, None);
        assert_eq!(extra.spent.0, 250);
    }

    #[test]
    fn implausible_decimal_places_is_schema_drift() {
        // A wire scale outside 0..=6 would mis-state every formatted amount
        // (10^100 overflows outright), so it fails loudly instead.
        for value in ["7", "-1", "100", "2.5"] {
            let raw = format!(
                r#"{{"extra_usage":{{"is_enabled":true,"used_credits":250,"decimal_places":{value}}}}}"#
            );
            assert!(
                serde_json::from_str::<UsageResponse>(&raw).is_err(),
                "{raw}"
            );
        }
        // An integral float is fine — this endpoint floats its numbers (the
        // #30 payload has `used_credits: 14157.0`), and rejecting 2.0 would
        // fail the whole response over an unambiguous value. Worse: the fetch
        // caches the body BEFORE parsing, so a rejected response would evict
        // the last good payload and leave a persistent ⚠.
        let raw = r#"{"extra_usage":{"is_enabled":true,"used_credits":250,"decimal_places":2.0}}"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(
            resp.into_snapshot("Pro".into())
                .extra
                .unwrap()
                .decimal_places,
            Some(2)
        );
        // Null and absent stay absent. With no currency, legacy payloads still
        // render using their historical USD/cent convention.
        let raw = r#"{"extra_usage":{"is_enabled":true,"used_credits":250,"decimal_places":null}}"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let extra = resp.into_snapshot("Pro".into()).extra.unwrap();
        assert_eq!(extra.decimal_places, None);
        assert_eq!(extra.fmt_spent(), "$2.50");

        // If a currency is present without its exponent, expose the raw minor
        // units rather than guessing. KRW is zero-decimal and KWD is
        // three-decimal; a blanket cent fallback corrupts both.
        let raw = r#"{"extra_usage":{"is_enabled":true,"used_credits":500,"currency":"KRW"}}"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let extra = resp.into_snapshot("Pro".into()).extra.unwrap();
        assert_eq!(extra.decimal_places, None);
        assert_eq!(extra.fmt_spent(), "500 minor units KRW");

        let raw = r#"{"extra_usage":{"is_enabled":true,"used_credits":1234,"currency":"KWD"}}"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let extra = resp.into_snapshot("Pro".into()).extra.unwrap();
        assert_eq!(extra.decimal_places, None);
        assert_eq!(extra.fmt_spent(), "1234 minor units KWD");

        // Explicit exponents remain authoritative, including zero and three.
        let raw = r#"{"extra_usage":{"is_enabled":true,"used_credits":500,"currency":"KRW","decimal_places":0}}"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(
            resp.into_snapshot("Pro".into()).extra.unwrap().fmt_spent(),
            "500 KRW"
        );
        let raw = r#"{"extra_usage":{"is_enabled":true,"used_credits":1234,"currency":"KWD","decimal_places":3}}"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(
            resp.into_snapshot("Pro".into()).extra.unwrap().fmt_spent(),
            "1.234 KWD"
        );
    }

    #[test]
    fn currency_must_be_an_iso_alpha_code() {
        // The value lands verbatim in Pango markup and in the `;;`-delimited
        // desktop FORMAT protocol, so anything but three ASCII uppercase
        // letters is rejected as drift — it is an injection vector besides.
        for value in [r#""brl""#, r#""""#, r#""R$""#, r#""USD;;0""#, r#""<b>""#] {
            let raw = format!(
                r#"{{"extra_usage":{{"is_enabled":true,"used_credits":250,"currency":{value}}}}}"#
            );
            assert!(
                serde_json::from_str::<UsageResponse>(&raw).is_err(),
                "{raw}"
            );
        }
        // Null stays acceptable — same as absent.
        let raw = r#"{"extra_usage":{"is_enabled":true,"used_credits":250,"currency":null}}"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(
            resp.into_snapshot("Pro".into()).extra.unwrap().currency,
            None
        );
    }

    #[test]
    fn malformed_cent_values_are_schema_drift() {
        for value in ["-1", "1.5", "1e300", "true", r#""lots""#] {
            let raw = format!(
                r#"{{"extra_usage":{{"is_enabled":true,"monthly_limit":{value},"used_credits":0}}}}"#
            );
            assert!(
                serde_json::from_str::<UsageResponse>(&raw).is_err(),
                "{raw}"
            );
        }

        let resp: UsageResponse = serde_json::from_str(
            r#"{"extra_usage":{"is_enabled":true,"monthly_limit":5000.0,"used_credits":250.0}}"#,
        )
        .unwrap();
        let extra = resp.into_snapshot("Pro".into()).extra.unwrap();
        assert_eq!(extra.limit, Some(Cents(5000)));
        assert_eq!(extra.spent.0, 250);
    }

    #[test]
    fn empty_object_yields_neutral_snapshot() {
        let resp: UsageResponse = serde_json::from_str("{}").unwrap();
        let snap = resp.into_snapshot("Unknown".into());
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 0);
        assert_eq!(snap.weekly.utilization_pct, 0);
        assert!(snap.session.as_ref().unwrap().resets_at.is_none());
    }

    #[test]
    fn benign_overshoot_saturates_to_hundred() {
        // A hair over the cap is rounding noise, not drift — it must render as
        // a full bar, not break the widget.
        let raw = r#"{
            "five_hour": {"utilization": 100.4},
            "seven_day": {"utilization": 100.6}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 100);
        assert_eq!(snap.weekly.utilization_pct, 100); // rounds to 101, saturated
    }

    #[test]
    fn out_of_range_utilization_is_rejected_not_clamped() {
        for raw in [
            r#"{"five_hour": {"utilization": 500}}"#,
            r#"{"five_hour": {"utilization": -1}}"#,
            r#"{"seven_day": {"utilization": 101.5}}"#,
        ] {
            let err = serde_json::from_str::<UsageResponse>(raw).unwrap_err();
            assert!(err.to_string().contains("outside 0..=100"), "{raw}: {err}");
        }
    }

    #[test]
    fn out_of_range_scoped_percent_is_rejected() {
        // `limits[].percent` reaches the same bar via the synthesized Window.
        let raw = r#"{
            "limits": [{"kind": "weekly_scoped", "percent": 420,
                        "scope": {"model": {"display_name": "Fable"}}}]
        }"#;
        let err = serde_json::from_str::<UsageResponse>(raw).unwrap_err();
        assert!(err.to_string().contains("outside 0..=100"), "{err}");
    }

    #[test]
    fn non_finite_percentage_is_rejected() {
        // `f64::NAN as i32` is silently 0 — the fabricated number this gate
        // exists to prevent. JSON has no NaN literal, so drive it directly.
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(checked_percent::<serde_json::Error>(v).is_err(), "{v}");
        }
    }

    #[test]
    fn scoped_limit_without_a_percentage_is_dropped_not_zeroed() {
        // The gate rejects impossible numbers; an absent one is not a parse
        // failure. But it must not become a bar either: `unwrap_or(0.0)` drew a
        // confident "Fable 0%" under a real model name that the API never sent.
        let raw = r#"{
            "limits": [{"kind": "weekly_scoped", "percent": null,
                        "scope": {"model": {"display_name": "Fable"}}}]
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert!(
            snap.scoped.is_empty(),
            "a scoped limit with no percentage must not render a fabricated bar"
        );

        // A real percentage still produces the window, so the drop is targeted.
        let ok = r#"{
            "limits": [{"kind": "weekly_scoped", "percent": 84,
                        "scope": {"model": {"display_name": "Fable"}}}]
        }"#;
        let resp: UsageResponse = serde_json::from_str(ok).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert_eq!(snap.scoped[0].label, "Fable");
        assert_eq!(snap.scoped[0].window.utilization_pct, 84);
    }

    /// The `cedar_ember` block as the endpoint actually returned it on
    /// 2026-09-24 for a Team account holding the Opus 5.5 launch grant.
    const LAUNCH_GRANT: &str = r#"{
        "five_hour": {"utilization": 2, "resets_at": "2026-09-24T17:49:59Z"},
        "seven_day": {"utilization": 63, "resets_at": "2026-09-25T08:59:59Z"},
        "cedar_ember": {
            "eligible": true,
            "ineligible_reason": null,
            "at_limit": false,
            "exhausted": [],
            "grants": [{
                "id": "a-redemption-handle",
                "label": "Claude Opus 5.5 launch: one usage-limit reset for Team members",
                "resets_total": 1,
                "resets_left": 1,
                "starts_at": "2026-09-22T16:00:00+00:00",
                "ends_at": "2026-10-22T16:00:00+00:00",
                "clears": ["five_hour", "seven_day", "seven_day_overage_included"],
                "paused": false,
                "usable_now": true,
                "use_requires_limit": false,
                "percent_used": {"five_hour": 2, "seven_day": 63},
                "blocking": [],
                "arm": null
            }],
            "next_grant_id": "a-redemption-handle",
            "weekly_resets_at": "2026-09-25T09:00:00+00:00",
            "cooldown_until": null,
            "event_props": null
        }
    }"#;

    #[test]
    fn parses_banked_resets_from_the_cedar_ember_block() {
        let resp: UsageResponse = serde_json::from_str(LAUNCH_GRANT).unwrap();
        let snap = resp.into_snapshot("Max 20x".into());

        assert_eq!(snap.reset_credits.available, 1);
        assert_eq!(
            snap.reset_credits.credits[0].title.as_deref(),
            Some("Claude Opus 5.5 launch: one usage-limit reset for Team members")
        );
        assert_eq!(
            snap.reset_credits.next_expiry(),
            Some("2026-10-22T16:00:00Z".parse().unwrap())
        );
        // The rest of the payload is untouched by the added query parameter.
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 2);
        assert_eq!(snap.weekly.utilization_pct, 63);
    }

    /// `id` is what *spends* the grant. It must not survive parsing, so that
    /// no later cache write, tooltip, or error message can carry it.
    #[test]
    fn the_redemption_grant_id_never_leaves_the_parser() {
        let resp: UsageResponse = serde_json::from_str(LAUNCH_GRANT).unwrap();
        let snap = resp.into_snapshot("Max 20x".into());
        assert!(!format!("{snap:?}").contains("a-redemption-handle"));
    }

    /// An ineligible caller still gets a well-formed block. Counting its
    /// grants would put a reset on the bar that the account cannot redeem.
    #[test]
    fn an_ineligible_block_reports_no_resets() {
        let raw = r#"{
            "cedar_ember": {
                "eligible": false,
                "ineligible_reason": "cli_version",
                "grants": [{"id": "g", "resets_left": 1, "usable_now": true,
                            "ends_at": "2026-10-22T16:00:00Z"}]
            }
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        assert!(resp.into_snapshot("Pro".into()).reset_credits.is_empty());
    }

    /// Three ways a grant can sit in the array without being yours to use:
    /// not yet open (`usable_now: false`, which is how a future `starts_at`
    /// arrives), paused mid-campaign, or already spent down to zero.
    #[test]
    fn only_redeemable_grants_are_counted() {
        let raw = r#"{
            "cedar_ember": {
                "eligible": true,
                "grants": [
                    {"id": "a", "label": "not open yet", "resets_left": 1,
                     "usable_now": false, "paused": false},
                    {"id": "b", "label": "paused", "resets_left": 1,
                     "usable_now": true, "paused": true},
                    {"id": "c", "label": "spent", "resets_left": 0,
                     "usable_now": true, "paused": false},
                    {"id": "d", "label": "yours", "resets_left": 2,
                     "usable_now": true, "paused": false,
                     "ends_at": "2026-10-22T16:00:00Z"}
                ]
            }
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let credits = resp.into_snapshot("Pro".into()).reset_credits;
        assert_eq!(credits.available, 2, "only the redeemable grant counts");
        assert_eq!(credits.credits.len(), 1);
        assert_eq!(credits.credits[0].title.as_deref(), Some("yours"));
    }

    /// Most accounts, most of the time. An absent or null block is the normal
    /// answer, not drift, so it must not fail the whole response.
    #[test]
    fn an_absent_or_null_block_is_not_a_parse_failure() {
        for raw in [
            r#"{"five_hour": {"utilization": 5}}"#,
            r#"{"cedar_ember": null}"#,
        ] {
            let resp: UsageResponse = serde_json::from_str(raw).unwrap();
            assert!(resp.into_snapshot("Pro".into()).reset_credits.is_empty());
        }
    }

    /// The label is rendered verbatim into Pango markup and the desktop
    /// FORMAT protocol. A hostile one is dropped without taking the expiry —
    /// the only actionable part — down with it.
    #[test]
    fn a_control_bearing_label_is_dropped_but_the_expiry_survives() {
        let raw = r#"{
            "cedar_ember": {
                "eligible": true,
                "grants": [{"id": "a", "label": "line\u0000break", "resets_left": 1,
                            "usable_now": true, "ends_at": "2026-10-22T16:00:00Z"}]
            }
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let credits = resp.into_snapshot("Pro".into()).reset_credits;
        assert_eq!(credits.available, 1);
        assert!(credits.credits[0].title.is_none());
        assert!(credits.credits[0].expires_at.is_some());
    }

    #[test]
    fn unparseable_reset_becomes_none() {
        let raw = r#"{
            "five_hour": {"utilization": 50, "resets_at": "not a date"},
            "seven_day": {"utilization": 0}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert!(snap.session.as_ref().unwrap().resets_at.is_none());
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 50);
    }

    /// The two credit blocks as the endpoint actually returned them on
    /// 2026-10-26 for a Max account with cloud sessions and no usage-credit
    /// balance: `iguana_necktie` holds the cloud session credits, `spend`
    /// states `balance: null` even though the credits are enabled.
    const CREDITS_PAYLOAD: &str = r#"{
        "five_hour": {"utilization": 12, "resets_at": "2026-10-26T15:00:00Z"},
        "seven_day": {"utilization": 34, "resets_at": "2026-11-02T07:00:00Z"},
        "iguana_necktie": {"utilization":0.0,"resets_at":"2026-11-05T07:59:00+00:00","limit_dollars":250,"used_dollars":0.0,"remaining_dollars":250.0,"locked_reason":null},
        "spend": {"used":{"amount_minor":0,"currency":"USD","exponent":2},"limit":{"amount_minor":0,"currency":"USD","exponent":2},"percent":0,"severity":"normal","enabled":false,"disabled_reason":"org_level_disabled_until","cap":{"money":null,"credits":{"amount_minor":0,"exponent":2}},"balance":null,"auto_reload":null,"disclaimer":"Usage credits cover you when you hit your plan limits. [Learn more](https://support.claude.com/articles/12429409)","can_purchase_credits":false,"can_toggle":false}
    }"#;

    #[test]
    fn the_real_credits_payload_lifts_cloud_credits_and_no_usage_balance() {
        let snap = serde_json::from_str::<UsageResponse>(CREDITS_PAYLOAD)
            .unwrap()
            .into_snapshot("Max 5x".into());
        let cloud = snap
            .cloud_credits
            .as_ref()
            .expect("iguana_necktie must lift into cloud_credits");
        assert_eq!(cloud.limit, Cents(25000));
        assert_eq!(cloud.remaining, Cents(25000));
        assert_eq!(
            cloud.expires_at,
            Some("2026-11-05T07:59:00Z".parse().unwrap())
        );
        assert!(!cloud.locked);
        // `spend.balance` is null: a balance the response does not report is
        // absent, never a fabricated $0.00.
        assert!(snap.usage_credits.is_none());
        // The windows keep parsing around the two new blocks.
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 12);
        assert_eq!(snap.weekly.utilization_pct, 34);
    }

    #[test]
    fn cloud_credits_carry_the_remaining_after_spend() {
        // 100 of 250 spent → 150 left: the remaining, not the used, is what
        // the panel meters against the limit.
        let raw = r#"{
            "iguana_necktie": {"resets_at": "2026-11-05T07:59:00+00:00",
                               "limit_dollars": 250, "used_dollars": 100,
                               "remaining_dollars": 150, "locked_reason": null}
        }"#;
        let snap = serde_json::from_str::<UsageResponse>(raw)
            .unwrap()
            .into_snapshot("Max 5x".into());
        let cloud = snap.cloud_credits.as_ref().unwrap();
        assert_eq!(cloud.limit, Cents(25000));
        assert_eq!(cloud.remaining, Cents(15000));
    }

    #[test]
    fn iguana_necktie_drift_degrades_to_none_not_an_error() {
        // Absent, null, a zero limit, a negative remaining, a field of the
        // wrong type, and a block of the wrong type all yield `None` — and
        // the rest of the response keeps parsing in every case, because the
        // cache holds the last good payload and a rejected response would
        // evict it.
        for raw in [
            r#"{"five_hour": {"utilization": 5}}"#,
            r#"{"five_hour": {"utilization": 5}, "iguana_necktie": null}"#,
            r#"{"iguana_necktie": {"limit_dollars": 0, "remaining_dollars": 250}}"#,
            r#"{"iguana_necktie": {"limit_dollars": 250, "remaining_dollars": -5}}"#,
            r#"{"iguana_necktie": {"limit_dollars": "a lot", "remaining_dollars": 250}}"#,
            r#"{"iguana_necktie": {"limit_dollars": 250, "remaining_dollars": null}}"#,
            r#"{"iguana_necktie": [1, 2]}"#,
            r#"{"iguana_necktie": "hostile"}"#,
        ] {
            let resp: UsageResponse =
                serde_json::from_str(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            assert!(
                resp.into_snapshot("Pro".into()).cloud_credits.is_none(),
                "{raw}"
            );
        }
        // A hostile block must not take the windows down with it.
        let resp: UsageResponse = serde_json::from_str(
            r#"{"five_hour": {"utilization": 5}, "iguana_necktie": "hostile"}"#,
        )
        .unwrap();
        assert_eq!(
            resp.into_snapshot("Pro".into())
                .session
                .as_ref()
                .unwrap()
                .utilization_pct,
            5
        );
    }

    #[test]
    fn a_non_empty_locked_reason_locks_the_credits_without_leaving_the_parser() {
        // The reason text is untrusted terminal input; only the boolean lock
        // crosses the parse boundary. A `<b>`-bearing reason must come back
        // as `locked: true` and nowhere else.
        let raw = r#"{
            "iguana_necktie": {"resets_at": "2026-11-05T07:59:00Z",
                               "limit_dollars": 250, "remaining_dollars": 250,
                               "locked_reason": "<b>org_migrating</b>"}
        }"#;
        let snap = serde_json::from_str::<UsageResponse>(raw)
            .unwrap()
            .into_snapshot("Max 5x".into());
        let cloud = snap.cloud_credits.as_ref().unwrap();
        assert!(cloud.locked);
        assert!(!format!("{cloud:?}").contains("<b>"));

        // An empty string is not a lock; a wrong-typed reason is not one
        // either — the block still lifts on its money fields.
        for (reason, locked) in [("\"\"", false), ("42", false), ("[1]", false)] {
            let raw = format!(
                r#"{{"iguana_necktie": {{"limit_dollars": 250, "remaining_dollars": 250, "locked_reason": {reason}}}}}"#
            );
            let snap = serde_json::from_str::<UsageResponse>(&raw)
                .unwrap()
                .into_snapshot("Max 5x".into());
            assert_eq!(snap.cloud_credits.as_ref().unwrap().locked, locked, "{raw}");
        }

        // An unparseable resets_at loses the expiry, not the credits.
        let raw = r#"{"iguana_necktie": {"limit_dollars": 250, "remaining_dollars": 250,
                                           "resets_at": "not a date"}}"#;
        let snap = serde_json::from_str::<UsageResponse>(raw)
            .unwrap()
            .into_snapshot("Max 5x".into());
        let cloud = snap.cloud_credits.as_ref().unwrap();
        assert!(cloud.expires_at.is_none());
        assert_eq!(cloud.limit, Cents(25000));
    }

    #[test]
    fn spend_balance_shape_a_lifts_minor_units_with_its_own_scale() {
        let raw = r#"{
            "spend": {"balance": {"amount_minor": 1500, "currency": "USD", "exponent": 2},
                      "auto_reload": {"enabled": true}}
        }"#;
        let usage = serde_json::from_str::<UsageResponse>(raw)
            .unwrap()
            .into_snapshot("Pro".into())
            .usage_credits
            .expect("shape (a) must lift into usage_credits");
        assert_eq!(usage.balance, Cents(1500));
        assert_eq!(usage.decimal_places, 2);
        assert_eq!(usage.currency.as_deref(), Some("USD"));
        // `spend.auto_reload.enabled` is the first source for the flag.
        assert_eq!(usage.auto_reload, Some(true));

        // This endpoint floats its numbers: an integral float is the same
        // shape, and a zero-exponent currency must not be divided by 100.
        let raw = r#"{
            "spend": {"balance": {"amount_minor": 500.0, "currency": "KRW", "exponent": 0.0},
                      "auto_reload": null}
        }"#;
        let usage = serde_json::from_str::<UsageResponse>(raw)
            .unwrap()
            .into_snapshot("Pro".into())
            .usage_credits
            .unwrap();
        assert_eq!(usage.balance, Cents(500));
        assert_eq!(usage.decimal_places, 0);
        assert_eq!(usage.currency.as_deref(), Some("KRW"));
        assert_eq!(usage.auto_reload, None);
    }

    #[test]
    fn spend_balance_shape_b_lifts_usd_minor_units_and_nested_auto_reload() {
        // Shape (b): `amount` in minor units, whose scale is only knowable
        // for USD — and the auto_reload flag nested in the balance itself,
        // which is the source reached when the top-level one is not a
        // boolean object.
        let raw = r#"{
            "spend": {"balance": {"amount": 750, "currency": "USD",
                                  "auto_reload_settings": {"enabled": false}},
                      "auto_reload": null}
        }"#;
        let usage = serde_json::from_str::<UsageResponse>(raw)
            .unwrap()
            .into_snapshot("Pro".into())
            .usage_credits
            .expect("shape (b) must lift into usage_credits");
        assert_eq!(usage.balance, Cents(750));
        assert_eq!(usage.decimal_places, 2);
        assert_eq!(usage.currency.as_deref(), Some("USD"));
        assert_eq!(usage.auto_reload, Some(false));

        // The top-level flag wins when both sources are booleans.
        let raw = r#"{
            "spend": {"balance": {"amount": 750, "currency": "USD",
                                  "auto_reload_settings": {"enabled": false}},
                      "auto_reload": {"enabled": true}}
        }"#;
        let usage = serde_json::from_str::<UsageResponse>(raw)
            .unwrap()
            .into_snapshot("Pro".into())
            .usage_credits
            .unwrap();
        assert_eq!(usage.auto_reload, Some(true));

        // An object that is not complete shape (a) (no `exponent`) but is
        // complete shape (b) takes (b): the shapes are tested independently,
        // a complete (a) wins, and only then is the object dropped.
        let raw = r#"{"spend": {"balance": {"amount_minor": 5, "amount": 100,
                                          "currency": "USD"}}}"#;
        let usage = serde_json::from_str::<UsageResponse>(raw)
            .unwrap()
            .into_snapshot("Pro".into())
            .usage_credits
            .unwrap();
        assert_eq!(usage.balance, Cents(100));
        assert_eq!(usage.decimal_places, 2);
    }

    #[test]
    fn spend_balances_we_cannot_express_are_absent_not_zero() {
        // None of these may become a $0.00 row: the response did not report
        // a balance in a shape we can state.
        for raw in [
            // The observed real shape: `balance: null`.
            r#"{"spend": {"balance": null}}"#,
            // Shape (b) in a non-USD currency: no scale we can vouch for.
            r#"{"spend": {"balance": {"amount": 500, "currency": "BRL"}}}"#,
            // Shape (a) with an exponent outside 0..=4.
            r#"{"spend": {"balance": {"amount_minor": 5, "currency": "USD", "exponent": 5}}}"#,
            // A number is not a balance object.
            r#"{"spend": {"balance": 42}}"#,
            // An unknown shape.
            r#"{"spend": {"balance": {"dollars": 4}}}"#,
            // Shape (a) missing its currency.
            r#"{"spend": {"balance": {"amount_minor": 5, "exponent": 2}}}"#,
            // A non-boolean `enabled` at either source is not a flag.
            r#"{"spend": {"balance": {"amount": 750, "currency": "USD",
                                  "auto_reload_settings": {"enabled": "on"}},
                      "auto_reload": 7}}"#,
        ] {
            let resp: UsageResponse =
                serde_json::from_str(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            let snap = resp.into_snapshot("Pro".into());
            if raw.contains(r#""auto_reload": 7"#) {
                // The balance still lifts; only the untyped flag is dropped.
                let usage = snap.usage_credits.as_ref().unwrap();
                assert_eq!(usage.auto_reload, None, "{raw}");
            } else {
                assert!(snap.usage_credits.is_none(), "{raw}");
            }
        }
    }

    #[test]
    fn spend_drift_degrades_to_none_not_an_error() {
        // A `spend` of the wrong type, or with hostile field values, must
        // not fail the whole response.
        for raw in [
            r#"{"five_hour": {"utilization": 5}}"#,
            r#"{"spend": null}"#,
            r#"{"spend": [1]}"#,
            r#"{"spend": "hostile"}"#,
            r#"{"spend": {"balance": "not a balance", "auto_reload": {"enabled": "on"}}}"#,
        ] {
            let resp: UsageResponse =
                serde_json::from_str(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            let snap = resp.into_snapshot("Pro".into());
            assert!(snap.usage_credits.is_none(), "{raw}");
        }
        let resp: UsageResponse =
            serde_json::from_str(r#"{"five_hour": {"utilization": 5}, "spend": "hostile"}"#)
                .unwrap();
        assert_eq!(
            resp.into_snapshot("Pro".into())
                .session
                .as_ref()
                .unwrap()
                .utilization_pct,
            5
        );
    }

    // ----------------------------------------------------------------
    // Numbers the parser cannot materialize degrade the block, not the
    // response.
    // ----------------------------------------------------------------

    #[test]
    fn a_number_the_parser_cannot_materialize_loses_only_its_block() {
        // 1e400 overflows every materialized number type serde_json has.
        // Captured raw, it costs its own block and nothing else: the five
        // hour window and the rest of the answer still parse.
        let raw = r#"{"five_hour":{"utilization":5},"iguana_necktie":{"limit_dollars":1e400,"remaining_dollars":1},"spend":{"balance":{"amount_minor":1e400,"currency":"USD","exponent":2}}}"#;
        let resp: UsageResponse =
            serde_json::from_str(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
        let snap = resp.into_snapshot("Pro".into());
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 5);
        assert!(snap.cloud_credits.is_none(), "1e400 dollars is not a limit");
        assert!(
            snap.usage_credits.is_none(),
            "1e400 minor units is not a balance"
        );
    }

    #[test]
    fn a_huge_finite_float_inside_a_credit_block_degrades_the_block_only() {
        // 1e300 is finite but far past the i64 cents range: the block's
        // re-parse rejects it and the rest of the answer stands.
        let raw = r#"{"five_hour":{"utilization":5},"iguana_necktie":{"limit_dollars":1e300,"remaining_dollars":1}}"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 5);
        assert!(snap.cloud_credits.is_none());
    }

    // ----------------------------------------------------------------
    // Dollars → cents in exact arithmetic, one function.
    // ----------------------------------------------------------------

    #[test]
    fn dollars_to_cents_rounds_monetary_ties_up_in_exact_arithmetic() {
        // (f64 * 100.0).round() cannot prove these: 1.005 dollars is 100.5
        // binary cents, which rounds to 100, not the 101 the money says.
        fn cents(text: &str) -> i64 {
            let n: serde_json::Number = serde_json::from_str(text).unwrap();
            dollars_to_cents(&n).unwrap()
        }
        assert_eq!(cents("1.005"), 101);
        assert_eq!(cents("0.004"), 0);
        assert_eq!(cents("149.995"), 15000);
        assert_eq!(cents("250"), 25000);
        assert_eq!(cents("0.005"), 1);
        assert_eq!(cents("19.99"), 1999);
    }

    #[test]
    fn dollars_to_cents_reads_negative_zero_as_an_exhausted_zero() {
        for text in ["-0", "-0.0"] {
            let n: serde_json::Number = serde_json::from_str(text).unwrap();
            assert_eq!(dollars_to_cents(&n), Some(0), "{text}");
        }
        let n: serde_json::Number = serde_json::from_str("-0.01").unwrap();
        assert_eq!(dollars_to_cents(&n), None);
    }

    #[test]
    fn dollars_to_cents_rejects_what_cents_cannot_hold() {
        fn opt(text: &str) -> Option<i64> {
            serde_json::from_str::<serde_json::Number>(text)
                .ok()
                .and_then(|n| dollars_to_cents(&n))
        }
        // A negative limit is drift, not a balance the meter can pace.
        assert_eq!(opt("-5"), None);
        // u64::MAX dollars overflows the integer build of the cents.
        assert_eq!(opt("18446744073709551615"), None);
        // Past the size gate: 1e13 dollars is 1e15 cents, still inside i64;
        // 1e14 dollars is rejected by the gate, not by an overflow.
        assert_eq!(opt("1e13"), Some(1_000_000_000_000_000));
        assert_eq!(opt("1e14"), None);
    }

    #[test]
    fn a_positive_dollar_limit_that_rounds_to_zero_cents_does_not_stand() {
        // 0.004 dollars is 0.4 cents: converted, it rounds to zero, and a
        // zero-cent limit must not stand — the meter would divide by zero
        // and read a confident 0% for a fully spent grant.
        let raw = r#"{"iguana_necktie":{"limit_dollars":0.004,"remaining_dollars":0.001,"resets_at":"2026-11-05T07:59:00Z"}}"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        assert!(
            resp.into_snapshot("Pro".into()).cloud_credits.is_none(),
            "a limit the conversion rounds to zero is absent, not a 0% meter"
        );
    }

    #[test]
    fn json_int_rejects_integers_past_its_own_range() {
        // u64::MAX is a legal JSON number and a legal minor-unit count in
        // principle — converting it to i64 cents would silently wrap to a
        // negative balance.
        assert_eq!(json_int(&serde_json::json!(18446744073709551615u64)), None);
        // The integral float the endpoint really sends still passes.
        assert_eq!(json_int(&serde_json::json!(1500.0)), Some(1500));
    }

    // ----------------------------------------------------------------
    // auto_reload falls through a non-boolean primary.
    // ----------------------------------------------------------------

    #[test]
    fn a_non_boolean_primary_auto_reload_falls_to_the_balance_setting() {
        // The top level reported a word, not a boolean: that is not a
        // signal, and the balance's own setting still stands.
        let raw = r#"{"spend":{"balance":{"amount":750,"currency":"USD","auto_reload_settings":{"enabled":true}},"auto_reload":{"enabled":"unknown"}}}"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        let credits = snap
            .usage_credits
            .expect("the balance is still shape (b) with a USD amount");
        assert_eq!(credits.balance, Cents(750));
        assert_eq!(credits.auto_reload, Some(true));
    }
}
