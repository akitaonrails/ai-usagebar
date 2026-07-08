//! Canonical in-memory representation of "how much have I used my plan".
//!
//! Each vendor's snapshot lives in its own variant — this is deliberate.
//! Anthropic exposes three windows + extra credits; OpenAI Codex exposes two
//! windows + credit balance + message-count ranges; OpenRouter is a single
//! credit-balance number with daily/weekly/monthly totals; Z.AI is a list of
//! token + MCP buckets. Forcing them into a shared shape would either drop
//! information or paper over genuine differences.
//!
//! Renderers (widget tooltip, TUI tab) consume a `VendorSnapshot` directly,
//! not a flattened shape — so each vendor controls its own presentation while
//! sharing the pacing math, color thresholds, and Pango primitives.

use chrono::{DateTime, Utc};

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

/// Money expressed in cents to dodge float roundoff.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtraUsage {
    pub limit: Cents,
    pub spent: Cents,
}

impl ExtraUsage {
    /// Integer percentage of the monthly limit consumed (0..=100, saturating
    /// at 0 when limit is non-positive — matches claudebar:540-542).
    pub fn percent(self) -> i32 {
        if self.limit.0 <= 0 {
            0
        } else {
            ((self.spent.0 * 100) / self.limit.0) as i32
        }
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
    /// The currency of the above amounts ("USD", "CNY", etc.).
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

/// Discriminated union of vendor-specific snapshots. The widget and TUI match
/// on this to pick a renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VendorSnapshot {
    Anthropic(AnthropicSnapshot),
    Openai(OpenAiSnapshot),
    Zai(ZaiSnapshot),
    Openrouter(OpenRouterSnapshot),
    Deepseek(DeepseekSnapshot),
}

/// OpenAI Codex OAuth — mirrors Anthropic's two-window + extras pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiSnapshot {
    pub plan: String,
    /// 5h window (Codex `rate_limit.primary_window`).
    pub session: UsageWindow,
    /// 7d window (Codex `rate_limit.secondary_window`).
    pub weekly: UsageWindow,
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
    if any_at_cap && let Some(extra) = snap.extra {
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
                limit: Cents(limit),
                spent: Cents(spent),
            }),
        }
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
                limit: Cents(0),
                spent: Cents(100)
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
                limit: Cents(10000),
                spent: Cents(3333)
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
}
