//! Canonical in-memory representation of "how much have I used my plan".
//!
//! Each vendor's snapshot lives in its own variant — this is deliberate.
//! Anthropic exposes three windows + extra credits; OpenAI Codex exposes up to
//! two windows + credit balance + message-count ranges; OpenRouter is a single
//! credit-balance number with daily/weekly/monthly totals; Z.AI is a list of
//! token + MCP buckets; DeepSeek is a credit balance; Kimi is a weekly quota
//! plus a 5h rolling rate-limit window. Forcing them into a shared shape would
//! either drop information or paper over genuine differences.
//!
//! Renderers (widget tooltip, TUI tab) consume a `VendorSnapshot` directly,
//! not a flattened shape — so each vendor controls its own presentation while
//! sharing the pacing math, color thresholds, and Pango primitives.

use chrono::{DateTime, Utc};

use crate::error::{AppError, Result};

/// Reject a non-finite monetary value. A NaN or infinity reaching a balance
/// field means the payload was not what we think it is; displaying it as money
/// (or caching it as authoritative) is worse than failing loudly.
pub fn finite_amount(vendor: &str, field: &str, v: f64) -> Result<f64> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(AppError::Schema(format!(
            "{vendor}: `{field}` is not a finite number"
        )))
    }
}

/// Parse a monetary field that the wire encodes as a string. A malformed or
/// empty value is a schema error, **not** a zero balance — silently reporting
/// $0.00 for an error envelope is the failure mode this guards against.
pub fn parse_amount(vendor: &str, field: &str, s: &str) -> Result<f64> {
    let t = s.trim();
    if t.is_empty() {
        return Err(AppError::Schema(format!("{vendor}: `{field}` is empty")));
    }
    let v: f64 = t
        .parse()
        .map_err(|_| AppError::Schema(format!("{vendor}: `{field}` is not numeric (got {t:?})")))?;
    finite_amount(vendor, field, v)
}

/// A single usage window — generic enough that every vendor with a notion of
/// "% used vs. when does it reset" can express itself with it.
///
/// `utilization_pct` is `0..=100` (integer percent, matching claudebar's units).
/// `resets_at` is `None` when the vendor doesn't report a reset time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageWindow {
    pub utilization_pct: i32,
    pub resets_at: Option<DateTime<Utc>>,
    /// Window length (used for pacing math).
    pub window_duration: chrono::Duration,
}

/// Money in minor currency units (historically always cents; see
/// `ExtraUsage::decimal_places` for the actual scale) to dodge float roundoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cents(pub i64);

impl Cents {
    /// Format as `[-]$D.CC`. Negative values render `-$D.CC` (not `$-D.CC`),
    /// matching claudebar's `_fmt_dollars` (claudebar:532-537).
    pub fn fmt_dollars(self) -> String {
        let (sign, abs) = if self.0 < 0 {
            ("-", -self.0)
        } else {
            ("", self.0)
        };
        format!("{sign}${}.{:02}", abs / 100, abs % 100)
    }
}

/// Anthropic-specific snapshot — three rolling windows plus optional
/// pay-as-you-go credit balance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicSnapshot {
    /// "Claude Pro", "Claude Max 5x", "Claude Max 20x", etc.
    pub plan: String,
    pub session: UsageWindow,
    pub weekly: UsageWindow,
    /// Some vendors of Claude (Pro, some Max tiers) don't have a separate
    /// Sonnet bucket — in which case this is None.
    pub sonnet: Option<UsageWindow>,
    /// Model-scoped weekly windows from the newer `limits[]` array
    /// (`kind == "weekly_scoped"`), e.g. the Fable weekly cap. Labels come
    /// from the API (`scope.model.display_name`), so new models show up
    /// without a code change. Empty when the account has none.
    pub scoped: Vec<ScopedWindow>,
    /// `None` when `extra_usage.is_enabled` is false or the block is absent.
    pub extra: Option<ExtraUsage>,
}

/// A usage window scoped to a specific model, labeled by the API
/// (e.g. "Fable"). Weekly (7d) duration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedWindow {
    pub label: String,
    pub window: UsageWindow,
}

/// "Extra usage" pay-as-you-go block (claudebar's `extra_usage`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtraUsage {
    /// `None` when the payload carries no usable `monthly_limit` — an
    /// explicit null (observed for plans without a spending cap, e.g. Claude
    /// Pro, #30) or an absent field. Either way the spend is real and stays
    /// visible; only the limit is unreported, and the renderers say exactly
    /// that rather than inferring a plan tier from it.
    pub limit: Option<Cents>,
    pub spent: Cents,
    /// ISO code from the block (`"BRL"`, `"USD"`, …). `None` on older payloads
    /// that predate the field — formatted as `$` for back-compat, which was
    /// the only behaviour before the field existed.
    pub currency: Option<String>,
    /// Minor-unit digits from the block's `decimal_places` (BRL/USD = 2,
    /// JPY/KRW = 0). `None` means the wire did not report the scale. We keep
    /// that absence instead of guessing from an incomplete currency table.
    pub decimal_places: Option<u32>,
}

impl ExtraUsage {
    /// Integer percentage of the monthly limit consumed (0..=100, saturating
    /// at 0 when limit is non-positive — matches claudebar:540-542).
    ///
    /// With no cap there is no denominator, so no meaningful percentage
    /// exists; 0 keeps the bar and severity calm rather than inventing one.
    pub fn percent(&self) -> i32 {
        match self.limit {
            Some(l) if l.0 > 0 => ((self.spent.0 * 100) / l.0) as i32,
            _ => 0,
        }
    }

    pub fn fmt_spent(&self) -> String {
        self.fmt_amount(self.spent)
    }

    pub fn fmt_limit(&self) -> Option<String> {
        self.limit.map(|l| self.fmt_amount(l))
    }

    fn fmt_amount(&self, amount: Cents) -> String {
        match (self.decimal_places, self.currency.as_deref()) {
            (Some(decimal_places), currency) => fmt_minor(amount.0, decimal_places, currency),
            // Legacy payloads predate both fields and were always cents/USD.
            // Preserve that established behaviour only when neither field can
            // tell us otherwise.
            (None, None) => fmt_minor(amount.0, 2, None),
            // A currency code alone does not determine its ISO minor-unit
            // exponent. Keep the amount truthful instead of silently dividing
            // zero-, three-, or four-decimal currencies by the wrong scale.
            (None, Some(currency)) => fmt_minor_units(amount.0, currency),
        }
    }
}

fn fmt_minor_units(minor: i64, currency: &str) -> String {
    let sign = if minor < 0 { "-" } else { "" };
    format!("{sign}{} minor units {currency}", minor.unsigned_abs())
}

/// Format an amount in minor units with its own currency and scale. Rendering
/// R$ 141.57 as "$141.57" is a claim about the wrong currency — the same class
/// of defect as a fabricated number. Known codes get their symbol (mirroring
/// `deepseek::format_money`); anything else renders as `AMOUNT CODE`, which is
/// still truthful.
pub fn fmt_minor(minor: i64, decimal_places: u32, currency: Option<&str>) -> String {
    let scale = 10_u64.pow(decimal_places);
    // `unsigned_abs`, not negation: `-i64::MIN` overflows. Unreachable from
    // the wire (the parse gate rejects negatives) but this is a pub fn.
    let sign = if minor < 0 { "-" } else { "" };
    let abs = minor.unsigned_abs();
    let number = if decimal_places == 0 {
        format!("{abs}")
    } else {
        format!(
            "{}.{:0width$}",
            abs / scale,
            abs % scale,
            width = decimal_places as usize
        )
    };
    match currency {
        None | Some("USD") => format!("{sign}${number}"),
        Some("BRL") => format!("{sign}R${number}"),
        Some("EUR") => format!("{sign}€{number}"),
        Some("GBP") => format!("{sign}£{number}"),
        Some("JPY") | Some("CNY") => format!("{sign}¥{number}"),
        Some(other) => format!("{sign}{number} {other}"),
    }
}

/// DeepSeek — credit balance from `/user/balance`.
#[derive(Debug, Clone, PartialEq)]
pub struct DeepseekSnapshot {
    pub is_available: bool,
    /// Current balance (prefer USD, fallback to CNY).
    pub balance: f64,
    /// Free-granted credits component.
    pub granted: f64,
    /// Topped-up (purchased) credits component.
    pub topped_up: f64,
    /// The currency of the above amounts (currently "USD" or "CNY").
    pub currency: String,
}

impl Eq for DeepseekSnapshot {}

impl Default for DeepseekSnapshot {
    fn default() -> Self {
        Self {
            is_available: false,
            balance: 0.0,
            granted: 0.0,
            topped_up: 0.0,
            currency: String::new(),
        }
    }
}

/// Cursor — the two included-usage pools the dashboard shows, from the
/// undocumented `cursor.com/api/usage-summary` endpoint (the same one the
/// dashboard's own frontend calls), authenticated with the session token the
/// Cursor IDE wrote to its local `state.vscdb`.
///
/// Since Cursor's mid-2026 pricing, a plan's included compute is split into two
/// quota pools, each shown as a percentage: **Cursor Models** (Auto + Composer,
/// `autoPercentUsed`) and **Other Models** (named / third-party, `apiPercentUsed`).
/// Overflow past either pool falls to on-demand spend. Percentages are integers
/// (rounded from the wire floats) to match the dashboard and every other
/// vendor's integer-percent convention; they can exceed 100 when a pool is over
/// its included allowance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorSnapshot {
    /// Membership label, title-cased from `membershipType` (e.g. "Ultra").
    pub plan: String,
    /// "Cursor Models" pool — Auto + Composer (`autoPercentUsed`, rounded).
    pub auto_pct: i32,
    /// "Other Models" pool — named / third-party (`apiPercentUsed`, rounded).
    pub api_pct: i32,
    /// Overall included usage (`totalPercentUsed`, rounded) — the dashboard's
    /// "you've used N% of your included total usage" headline.
    pub total_pct: i32,
    /// `true` when the plan reports `isUnlimited` — the pools don't cap and the
    /// percentages are not meaningful.
    pub unlimited: bool,
    /// Whether on-demand (overage) spend is turned on (`onDemand.enabled`).
    pub on_demand_enabled: bool,
    /// End of the current billing cycle (`billingCycleEnd`) — when the pools
    /// reset.
    pub reset_at: Option<DateTime<Utc>>,
}

impl CursorSnapshot {
    /// The binding pool — whichever is closest to (or furthest past) its cap.
    /// Drives the bar color and the single generic `session_pct` alias.
    pub fn worst_pct(&self) -> i32 {
        self.auto_pct.max(self.api_pct)
    }
}

/// GitHub Copilot — quota from `api.github.com/copilot_internal/user`, the same
/// live account endpoint GitHub's own Copilot clients poll. One account can
/// expose up to three pools: chat, completions, and premium interactions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotSnapshot {
    /// GitHub login the quota belongs to. Used for cache isolation and shown in
    /// the tooltip; never used as an authentication secret.
    pub login: String,
    /// Human-facing plan label, title-cased from `copilot_plan`.
    pub plan: String,
    pub chat: CopilotPool,
    pub completions: CopilotPool,
    pub premium_interactions: CopilotPool,
    /// Quota reset time (`quota_reset_date_utc`).
    pub reset_at: Option<DateTime<Utc>>,
}

impl CopilotSnapshot {
    /// Copilot's user-facing headline pool is premium interactions when present;
    /// plans with no premium pool fall back to the most-exhausted applicable
    /// chat/completions pool.
    pub fn primary_pct(&self) -> Option<i32> {
        self.premium_interactions.percent_used().or_else(|| {
            [
                self.chat.percent_used(),
                self.completions.percent_used(),
                self.premium_interactions.percent_used(),
            ]
            .into_iter()
            .flatten()
            .max()
        })
    }

    pub fn secondary_pct(&self) -> Option<i32> {
        self.chat
            .percent_used()
            .or_else(|| self.completions.percent_used())
            .or_else(|| self.premium_interactions.percent_used())
    }
}

/// One Copilot quota pool. `NotApplicable` is distinct from `Unlimited`: the
/// former means this account simply has no such pool (`has_quota: false`), while
/// the latter means the product is uncapped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopilotPool {
    Metered {
        entitlement: u64,
        remaining: u64,
        percent_used: i32,
    },
    Unlimited,
    NotApplicable,
}

impl CopilotPool {
    pub fn percent_used(&self) -> Option<i32> {
        match self {
            Self::Metered { percent_used, .. } => Some(*percent_used),
            Self::Unlimited | Self::NotApplicable => None,
        }
    }
}

/// Kiro CLI (AWS CodeWhisperer / Q Developer backend) — a single credit pool
/// from `AmazonCodeWhispererService.GetUsageLimits`, the same call kiro-cli's
/// own `/usage` slash command makes. Authenticated with the AWS SSO OIDC
/// bearer token kiro-cli already cached locally, refreshed with the paired
/// refresh token when it's close to expiry — see `kiro::db` and `kiro::oauth`.
#[derive(Debug, Clone, PartialEq)]
pub struct KiroSnapshot {
    /// Subscription tier label (`subscriptionInfo.subscriptionTitle`, e.g.
    /// "KIRO POWER").
    pub plan: String,
    /// Credits consumed this cycle (`currentUsageWithPrecision`).
    pub used: f64,
    /// Credits included in the plan (`usageLimitWithPrecision`).
    pub limit: f64,
    /// When the credit pool resets (`nextDateReset`).
    pub reset_at: Option<DateTime<Utc>>,
}

impl Eq for KiroSnapshot {}

impl KiroSnapshot {
    /// Percentage of the credit pool consumed, rounded. `0` when `limit` is
    /// not positive — defensive; the API has not been observed to send that.
    pub fn pct(&self) -> i32 {
        if self.limit <= 0.0 {
            return 0;
        }
        ((self.used / self.limit) * 100.0)
            .round()
            .clamp(0.0, 9999.0) as i32
    }
}

/// Kimi Code — weekly subscription quota plus a 5h rolling rate-limit window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KimiSnapshot {
    pub plan: Option<String>,
    pub weekly_limit: u64,
    pub weekly_used: u64,
    pub weekly_remaining: u64,
    pub weekly_reset_at: Option<DateTime<Utc>>,
    pub window_limit: u64,
    pub window_used: u64,
    pub window_remaining: u64,
    pub window_reset_at: Option<DateTime<Utc>>,
}

impl KimiSnapshot {
    fn pct(used: u64, limit: u64) -> i32 {
        if limit == 0 {
            0
        } else {
            // Keep all quota values exact: f64 loses integer precision above
            // 2^53. This is the integer equivalent of round(used / limit *
            // 100), with saturation for inconsistent upstream counters.
            let pct = ((used as u128 * 100) + (limit as u128 / 2)) / limit as u128;
            pct.min(100) as i32
        }
    }

    /// Percentage of the weekly subscription quota consumed (0..=100).
    pub fn weekly_pct(&self) -> i32 {
        Self::pct(self.weekly_used, self.weekly_limit)
    }

    /// Percentage of the rolling rate-limit window consumed (0..=100).
    pub fn window_pct(&self) -> i32 {
        Self::pct(self.window_used, self.window_limit)
    }
}

/// Discriminated union of vendor-specific snapshots. The widget and TUI match
/// on this to pick a renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VendorSnapshot {
    Anthropic(AnthropicSnapshot),
    Openai(OpenAiSnapshot),
    Zai(ZaiSnapshot),
    Openrouter(OpenRouterSnapshot),
    Deepseek(DeepseekSnapshot),
    Kimi(KimiSnapshot),
    Kilo(KiloSnapshot),
    Novita(NovitaSnapshot),
    Moonshot(MoonshotSnapshot),
    Grok(GrokSnapshot),
    SuperGrok(SuperGrokSnapshot),
    AnthropicApi(AnthropicApiSnapshot),
    Antigravity(AntigravitySnapshot),
    Cursor(CursorSnapshot),
    Copilot(CopilotSnapshot),
    Minimax(MinimaxSnapshot),
    Kiro(KiroSnapshot),
    NousResearch(crate::nous::types::AccountSnapshot),
    OpenCodeGo(crate::opencode_go::types::Usage),
}

/// Google Antigravity 2.0 / CLI snapshot. The API groups models into Gemini
/// and third-party (Claude/GPT) buckets, and each group carries its own 5-hour
/// and weekly window — four independent windows in total.
#[derive(Debug, Clone, PartialEq)]
pub struct AntigravitySnapshot {
    pub plan: String,
    /// Fingerprint of the signed-in account. Never displayed — it exists so a
    /// cache written for one Google account is not served for another.
    pub account: String,
    /// Gemini group, 5-hour window.
    pub session: UsageWindow,
    /// Gemini group, weekly window.
    pub weekly: UsageWindow,
    /// Claude/GPT group, 5-hour window.
    pub third_party_session: Option<UsageWindow>,
    /// Claude/GPT group, weekly window.
    pub third_party_weekly: Option<UsageWindow>,
}

impl Eq for AntigravitySnapshot {}

/// MiniMax Token Plan — `/v1/token_plan/remains` returns one row per model
/// bucket (`general` for text/coding, `video`), and each row carries its own
/// rolling interval window plus a weekly window.
///
/// Two things the payload dictates rather than convention: the interval length
/// is **not fixed** (`general` rolls every 5h, `video` every 24h), so the
/// duration is derived from the row's own start/end rather than assumed; and
/// the API reports the percentage **remaining**, which is inverted on the way
/// in so these windows carry consumed-% like every other vendor's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinimaxSnapshot {
    pub plan: String,
    /// `general` bucket — rolling interval window (5h on the observed plans).
    pub session: UsageWindow,
    /// `general` bucket — weekly window.
    pub weekly: UsageWindow,
    /// `video` bucket, `None` on plans that carry no video quota.
    pub video_session: Option<UsageWindow>,
    pub video_weekly: Option<UsageWindow>,
}

/// Anthropic Admin API — month-to-date spend (USD) from the cost report. The
/// monthly `limit` is supplied from config (the API exposes neither the limit
/// nor the remaining prepaid credit balance).
#[derive(Debug, Clone, PartialEq)]
pub struct AnthropicApiSnapshot {
    pub spent: f64,
    pub limit: Option<f64>,
}

impl Eq for AnthropicApiSnapshot {}

impl AnthropicApiSnapshot {
    /// Spend as an integer percentage of the configured limit; `None` when no
    /// positive limit is set.
    pub fn pct(&self) -> Option<i32> {
        self.limit
            .filter(|l| l.is_finite() && *l > 0.0)
            .map(|l| ((self.spent / l) * 100.0).round().clamp(0.0, 9999.0) as i32)
    }
}

/// Kilo Code — remaining credit balance from `/api/profile/balance` (USD).
/// No purchased-total is exposed on that endpoint, so there's no consumed-%.
#[derive(Debug, Clone, PartialEq)]
pub struct KiloSnapshot {
    pub label: String,
    pub balance: f64,
}

impl Eq for KiloSnapshot {}

/// Novita AI — account balance from `/openapi/v1/billing/balance/detail`, with
/// all amounts already converted from the API's 1/10000-USD integers to USD.
#[derive(Debug, Clone, PartialEq)]
pub struct NovitaSnapshot {
    /// Spendable credit balance (`availableBalance`).
    pub available: f64,
    /// Remaining top-up (`cashBalance`).
    pub cash: f64,
    /// Credit limit — max you can owe (`creditLimit`).
    pub credit_limit: f64,
    /// Amount currently owed (`outstandingInvoices`).
    pub outstanding: f64,
}

impl Eq for NovitaSnapshot {}

/// Moonshot / Kimi — account balance from `/v1/users/me/balance`. Currency is
/// USD (`api.moonshot.ai`) or CNY (`api.moonshot.cn`); there's no currency
/// field in the response, so it's carried here from the region config.
#[derive(Debug, Clone, PartialEq)]
pub struct MoonshotSnapshot {
    /// Spendable balance (`available_balance` = cash + voucher). `<= 0` blocks
    /// the inference API.
    pub available: f64,
    /// Voucher credit (`voucher_balance`).
    pub voucher: f64,
    /// Cash balance (`cash_balance`); can be negative (debt).
    pub cash: f64,
    /// "USD" or "CNY", implied by the host.
    pub currency: String,
}

impl Eq for MoonshotSnapshot {}

/// xAI (Grok) — prepaid credit balance in USD, derived from the Management
/// API's `total.val` (USD cents, inverted-ledger; see `grok::types`).
#[derive(Debug, Clone, PartialEq)]
pub struct GrokSnapshot {
    pub balance: f64,
}

impl Eq for GrokSnapshot {}

/// SuperGrok subscription usage returned by the official Grok Build CLI's
/// credential-owning `x.ai/billing` ACP extension. Distinct from
/// [`GrokSnapshot`] (Management API prepaid balance).
#[derive(Debug, Clone, PartialEq)]
pub struct SuperGrokSnapshot {
    /// Subscription tier label when the billing response supplies one
    /// (e.g. "SuperGrok", "SuperGrok Heavy"); otherwise `"SuperGrok"`.
    pub plan: String,
    /// Opaque digest of Grok auth/config state. Never displayed — cache
    /// isolation only.
    pub account: String,
    /// Current included-credit usage percent. The field name is retained as a
    /// compatibility alias for format/render code; [`Self::period`] says
    /// whether the server's actual window is weekly or monthly.
    pub weekly_pct: i32,
    pub period: SuperGrokPeriod,
    /// When the current usage period ends.
    pub reset_at: Option<DateTime<Utc>>,
    /// Remaining prepaid (purchased) API credit in USD, when present.
    pub prepaid_balance: Option<f64>,
}

impl Eq for SuperGrokSnapshot {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuperGrokPeriod {
    Weekly,
    Monthly,
    Unknown,
}

impl SuperGrokPeriod {
    pub fn label(self) -> &'static str {
        match self {
            Self::Weekly => "Weekly",
            Self::Monthly => "Monthly",
            Self::Unknown => "Current period",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::Weekly => "wk",
            Self::Monthly => "mo",
            Self::Unknown => "period",
        }
    }
}

/// OpenAI Codex OAuth — exposes whichever rolling windows the API reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiSnapshot {
    pub plan: String,
    /// 5h window, identified by its duration rather than its wire position.
    pub session: Option<UsageWindow>,
    /// 7d window, identified by its duration rather than its wire position.
    pub weekly: Option<UsageWindow>,
    /// Optional 7d code-review bucket.
    pub code_review: Option<UsageWindow>,
    /// Optional credit balance + approximate message-count ranges.
    pub credits: Option<OpenAiCredits>,
    /// Source of the snapshot — Codex OAuth vs admin-key fallback. Drives
    /// the placeholder set and the "OpenAI does not expose this for Plus"
    /// tooltip when the OAuth path isn't available.
    pub source: OpenAiSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiSource {
    CodexOauth,
    AdminKeyMtd,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCredits {
    /// Credit balance, formatted dollars ("$0.00", "$5.00", etc.) — kept as
    /// a string because OpenAI returns it that way.
    pub balance: String,
    pub has_credits: bool,
    pub unlimited: bool,
    pub approx_local_messages: Option<(i64, i64)>,
    pub approx_cloud_messages: Option<(i64, i64)>,
}

/// Z.AI / BigModel — list of buckets with discriminated types. We project the
/// two we care about into named fields (5h tokens, weekly tokens, MCP).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZaiSnapshot {
    pub plan: String,
    pub session: Option<UsageWindow>,
    pub weekly: Option<UsageWindow>,
    pub mcp: Option<UsageWindow>,
}

/// OpenRouter — credit balance + lifetime/daily/weekly/monthly usage from
/// `/api/v1/credits` and `/api/v1/key`.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenRouterSnapshot {
    pub label: String,
    pub total_credits: f64,
    pub total_usage: f64,
    pub usage_daily: f64,
    pub usage_weekly: f64,
    pub usage_monthly: f64,
    pub is_free_tier: bool,
    pub limit: Option<f64>,
    pub limit_remaining: Option<f64>,
}

impl Eq for OpenRouterSnapshot {}

impl OpenRouterSnapshot {
    pub fn balance(&self) -> f64 {
        (self.total_credits - self.total_usage).max(0.0)
    }
    /// Percentage of total_credits consumed (0..=100). Returns 0 when
    /// `total_credits` is 0 (free-tier-only accounts).
    pub fn consumed_pct(&self) -> i32 {
        if self.total_credits <= 0.0 {
            return 0;
        }
        ((self.total_usage / self.total_credits) * 100.0)
            .round()
            .clamp(0.0, 100.0) as i32
    }
}

/// Worst-of severity class for the Waybar bar text color. Mirrors
/// claudebar:606-620 — "extra usage only matters when a rate limit hits 100%".
pub fn anthropic_severity(snap: &AnthropicSnapshot) -> crate::pacing::PaceSeverity {
    let mut max = snap.session.utilization_pct;
    if snap.weekly.utilization_pct > max {
        max = snap.weekly.utilization_pct;
    }
    if let Some(s) = &snap.sonnet
        && s.utilization_pct > max
    {
        max = s.utilization_pct;
    }
    for sw in &snap.scoped {
        if sw.window.utilization_pct > max {
            max = sw.window.utilization_pct;
        }
    }
    // Extra usage only promotes severity if a rate-limit window is at 100%.
    let any_at_cap = snap.session.utilization_pct >= 100
        || snap.weekly.utilization_pct >= 100
        || snap
            .sonnet
            .as_ref()
            .is_some_and(|s| s.utilization_pct >= 100)
        || snap.scoped.iter().any(|s| s.window.utilization_pct >= 100);
    if any_at_cap && let Some(extra) = snap.extra.as_ref() {
        let p = extra.percent();
        if p > max {
            max = p;
        }
    }
    crate::pango::severity_for(max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pacing::PaceSeverity;
    use chrono::Duration;

    fn w(pct: i32) -> UsageWindow {
        UsageWindow {
            utilization_pct: pct,
            resets_at: None,
            window_duration: Duration::hours(5),
        }
    }

    fn snap(s: i32, w_: i32, sonnet: Option<i32>, extra: Option<(i64, i64)>) -> AnthropicSnapshot {
        AnthropicSnapshot {
            plan: "Max 5x".into(),
            session: w(s),
            weekly: w(w_),
            sonnet: sonnet.map(w),
            scoped: vec![],
            extra: extra.map(|(limit, spent)| ExtraUsage {
                limit: Some(Cents(limit)),
                spent: Cents(spent),
                currency: None,
                decimal_places: Some(2),
            }),
        }
    }

    #[test]
    fn fmt_minor_honors_currency_and_scale() {
        // No currency (older payloads) keeps the historical `$`.
        assert_eq!(fmt_minor(250, 2, None), "$2.50");
        // The #30 reporter's actual figures: BRL must not be claimed as `$`.
        assert_eq!(fmt_minor(14157, 2, Some("BRL")), "R$141.57");
        assert_eq!(fmt_minor(14157, 2, Some("USD")), "$141.57");
        // Zero-exponent currency: no decimal point, no /100.
        assert_eq!(fmt_minor(500, 0, Some("JPY")), "¥500");
        // Sign precedes the symbol, matching `fmt_dollars`.
        assert_eq!(fmt_minor(-150, 2, Some("BRL")), "-R$1.50");
        // Unknown code stays truthful as a suffix rather than guessing a symbol.
        assert_eq!(fmt_minor(1234, 2, Some("CHF")), "12.34 CHF");
    }

    #[test]
    fn extra_usage_formats_in_its_own_currency() {
        let e = ExtraUsage {
            limit: None,
            spent: Cents(14157),
            currency: Some("BRL".into()),
            decimal_places: Some(2),
        };
        assert_eq!(e.fmt_spent(), "R$141.57");
        assert_eq!(e.fmt_limit(), None);

        let capped = ExtraUsage {
            limit: Some(Cents(5000)),
            spent: Cents(250),
            currency: None,
            decimal_places: Some(2),
        };
        assert_eq!(capped.fmt_spent(), "$2.50");
        assert_eq!(capped.fmt_limit().as_deref(), Some("$50.00"));
    }

    #[test]
    fn cents_format_positive() {
        assert_eq!(Cents(0).fmt_dollars(), "$0.00");
        assert_eq!(Cents(50).fmt_dollars(), "$0.50");
        assert_eq!(Cents(250).fmt_dollars(), "$2.50");
        assert_eq!(Cents(5000).fmt_dollars(), "$50.00");
    }

    #[test]
    fn cents_format_negative_uses_leading_sign() {
        // claudebar bug-fix: never "$-1.-50" — sign goes before the dollar sign.
        assert_eq!(Cents(-150).fmt_dollars(), "-$1.50");
        assert_eq!(Cents(-1).fmt_dollars(), "-$0.01");
    }

    #[test]
    fn extra_percent_with_zero_limit_is_zero() {
        assert_eq!(
            ExtraUsage {
                limit: Some(Cents(0)),
                spent: Cents(100),
                currency: None,
                decimal_places: Some(2),
            }
            .percent(),
            0
        );
    }

    #[test]
    fn extra_percent_truncates() {
        // Bash integer division — 33/100 -> 33%, 50/100 -> 50%.
        assert_eq!(
            ExtraUsage {
                limit: Some(Cents(10000)),
                spent: Cents(3333),
                currency: None,
                decimal_places: Some(2),
            }
            .percent(),
            33
        );
    }

    #[test]
    fn severity_picks_worst_of_three_windows() {
        let s = snap(40, 60, Some(80), None);
        assert_eq!(anthropic_severity(&s), PaceSeverity::High); // 80 → high
    }

    #[test]
    fn severity_ignores_extra_when_no_cap_hit() {
        // Extra at 95% but no rate-limit at 100% → extra is NOT promoted.
        let s = snap(50, 60, None, Some((10000, 9500)));
        assert_eq!(anthropic_severity(&s), PaceSeverity::Mid); // capped at 60
    }

    #[test]
    fn severity_promotes_extra_when_session_at_100() {
        let s = snap(100, 50, None, Some((10000, 9500)));
        assert_eq!(anthropic_severity(&s), PaceSeverity::Critical); // 100 → critical
    }

    #[test]
    fn severity_falls_through_to_extra_when_extra_higher_than_capped_window() {
        // session = 100, weekly = 50, extra = 100% → max should be 100.
        let s = snap(100, 50, None, Some((10000, 10000)));
        assert_eq!(anthropic_severity(&s), PaceSeverity::Critical);
    }

    fn with_scoped(mut s: AnthropicSnapshot, pct: i32) -> AnthropicSnapshot {
        s.scoped.push(ScopedWindow {
            label: "Fable".into(),
            window: w(pct),
        });
        s
    }

    #[test]
    fn severity_includes_scoped_windows() {
        // The PR #19 scenario: overall weekly at 55 (Mid) but a scoped Fable
        // week at 84 → the bar class must escalate to High.
        let s = with_scoped(snap(10, 55, None, None), 84);
        assert_eq!(anthropic_severity(&s), PaceSeverity::High);
    }

    #[test]
    fn severity_promotes_extra_when_scoped_at_100() {
        // A scoped window at cap counts as a rate-limit cap hit, so extra
        // usage above the window max is promoted — same rule as session/weekly.
        let s = with_scoped(snap(10, 50, None, Some((10000, 9900))), 100);
        assert_eq!(anthropic_severity(&s), PaceSeverity::Critical);
    }

    #[test]
    fn kimi_percent_is_exact_above_f64_precision() {
        let snap = KimiSnapshot {
            plan: None,
            weekly_limit: (1 << 53) + 1,
            weekly_used: 1 << 52,
            weekly_remaining: 0,
            weekly_reset_at: None,
            window_limit: u64::MAX,
            window_used: u64::MAX - 1,
            window_remaining: 0,
            window_reset_at: None,
        };
        assert_eq!(snap.weekly_pct(), 50);
        assert_eq!(snap.window_pct(), 100);
    }

    #[test]
    fn kiro_pct_is_zero_without_a_positive_limit() {
        let snap = KiroSnapshot {
            plan: "FREE".into(),
            used: 5.0,
            limit: 0.0,
            reset_at: None,
        };
        assert_eq!(snap.pct(), 0);
    }

    #[test]
    fn kiro_pct_rounds_the_credit_ratio() {
        let snap = KiroSnapshot {
            plan: "KIRO POWER".into(),
            used: 1.0,
            limit: 3.0,
            reset_at: None,
        };
        assert_eq!(snap.pct(), 33);
    }
}
