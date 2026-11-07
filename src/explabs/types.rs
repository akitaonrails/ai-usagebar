//! Experiential Labs wire types and their projection into
//! [`ExperientialLabsSnapshot`].
//!
//! Three reads carry every figure, all with the same `xpl_` Bearer key:
//! `GET /api/v1/credits` (the wallet), `GET /api/v1/key` (the presented key's
//! daily USD cap) and `GET /api/v1/usage?limit=200` (the settled per-request
//! export, newest first over a rolling 7-day window). Two units need care:
//!
//! - Token figures are **metered** tokens, input and output reported
//!   separately, so the daily meters are directly comparable with the shared
//!   daily pool and with each other.
//! - `cost_usd` is charged credits only; `estimated_cost_usd` is the
//!   catalog-rate value of attributed BYOK usage and is never folded into a
//!   charge.
//!
//! The daily meters reset at 00:00 UTC by design, so `period_end` is exact;
//! the export's window is rolling, so only the UTC-day slice of it projects
//! into the daily meters.

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::usage::ExperientialLabsSnapshot as Snapshot;
use crate::usage::{ExperientialLabsMeter, ExperientialLabsModelUsage};

/// The documented Free-tier shared pool: `1,000,000` input+output tokens per
/// UTC day. The gateway reports no token allowance on the wire, so this
/// documented ceiling stands in for one — exact on Free, and on paid plans
/// (which cap by credits/day instead) the wallet balance and the key's cap
/// are the binding figures the tooltip leads with.
pub const FREE_DAILY_TOKEN_LIMIT: i64 = 1_000_000;

/// `GET /api/v1/credits` on the same `xpl_` key. The response
/// wraps the figures in `data`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Credits {
    pub total_credits: f64,
    pub total_usage: f64,
}

impl Credits {
    /// Spendable credit, **which can be negative**: usage past the wallet is
    /// real, and clamping that to zero would report a healthy-looking `$0.00`
    /// to someone who has to top up before anything works again.
    pub fn balance(&self) -> f64 {
        self.total_credits - self.total_usage
    }
}

/// `GET /api/v1/key` — the presented key's own summary. `limit` is a daily
/// USD cap; both cap fields are null on a key with no cap.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KeyInfo {
    pub usage: f64,
    #[serde(default)]
    pub limit: Option<f64>,
    #[serde(default)]
    pub limit_remaining: Option<f64>,
}

/// One settled request from `GET /api/v1/usage?limit=200`. Rows never change
/// once settled, and `real_cost_usd = cost_usd + estimated_cost_usd` is
/// derivable, so only the fields the projection reads are kept.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct UsageEvent {
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    /// Charged credits, in dollars. `0` with `pricing_known: false` means
    /// unpriced, not free.
    #[serde(default)]
    pub cost_usd: f64,
}

/// What the vendor cache stores: the wallet read, the key's cap, and the
/// settled rows the daily meters and the per-model table project from.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CachedPayload {
    pub credits: Credits,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<KeyInfo>,
    #[serde(default)]
    pub events: Vec<UsageEvent>,
}

impl CachedPayload {
    pub fn snapshot(&self) -> Result<Snapshot> {
        let today = chrono::Utc::now().date_naive();
        let today_rows: Vec<&UsageEvent> = self
            .events
            .iter()
            .filter(|event| event.created_at.get(..10) == Some(today.to_string().as_str()))
            .collect();

        let daily_input = (!today_rows.is_empty()).then_some(ExperientialLabsMeter {
            used: today_rows.iter().map(|row| row.input_tokens).sum(),
            limit: FREE_DAILY_TOKEN_LIMIT,
        });
        let daily_output = (!today_rows.is_empty()).then_some(ExperientialLabsMeter {
            used: today_rows.iter().map(|row| row.output_tokens).sum(),
            limit: FREE_DAILY_TOKEN_LIMIT,
        });

        let mut models: Vec<ExperientialLabsModelUsage> = Vec::new();
        for event in &self.events {
            if event.model.is_empty() {
                continue;
            }
            if let Some(row) = models.iter_mut().find(|row| row.model == event.model) {
                row.input_tokens += event.input_tokens;
                row.output_tokens += event.output_tokens;
                row.cost_usd += event.cost_usd;
            } else {
                models.push(ExperientialLabsModelUsage {
                    model: event.model.clone(),
                    input_tokens: event.input_tokens,
                    output_tokens: event.output_tokens,
                    cost_usd: event.cost_usd,
                });
            }
        }
        models.sort_by(|a, b| {
            b.weighted_total()
                .total_cmp(&a.weighted_total())
                .then_with(|| a.model.cmp(&b.model))
        });

        let period_start = today.and_hms_opt(0, 0, 0).map(|start| start.and_utc());
        let period_end = today
            .succ_opt()
            .and_then(|end| end.and_hms_opt(0, 0, 0))
            .map(|end| end.and_utc());

        Ok(Snapshot {
            plan: String::new(),
            balance_usd: Some(self.credits.balance()),
            daily_input,
            daily_output,
            key_limit_usd: self.key.as_ref().and_then(|k| k.limit),
            key_limit_remaining_usd: self.key.as_ref().and_then(|k| k.limit_remaining),
            period_start,
            period_end,
            models,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload_with_row(created_at: &str, input: i64, output: i64) -> CachedPayload {
        CachedPayload {
            credits: Credits {
                total_credits: 200.0,
                total_usage: 175.5,
            },
            key: None,
            events: vec![UsageEvent {
                created_at: created_at.into(),
                model: "claude-xpl".into(),
                input_tokens: input,
                output_tokens: output,
                cost_usd: 1.0,
            }],
        }
    }

    #[test]
    fn only_todays_rows_fill_the_daily_meters() {
        let today = chrono::Utc::now().date_naive().to_string();
        let snapshot = payload_with_row(&format!("{today}T12:00:00Z"), 812_400, 187_600)
            .snapshot()
            .expect("snapshot");
        let input = snapshot.daily_input.expect("today's input meter");
        assert_eq!(input.used, 812_400);
        assert_eq!(input.limit, FREE_DAILY_TOKEN_LIMIT);
        let output = snapshot.daily_output.expect("today's output meter");
        assert_eq!(output.used, 187_600);
        // One shared pool: the headline figure is the combined draw, and the
        // day reset is the next 00:00 UTC.
        let pool = snapshot.daily_worst().expect("shared pool meter");
        assert_eq!(pool.used, 1_000_000);
        assert_eq!(pool.pct(), Some(100));
        assert_eq!(snapshot.daily_consumed_pct(), 100);
        assert_eq!(
            snapshot.period_start.map(|t| t.to_string()),
            Some(format!("{today} 00:00:00 UTC"))
        );
    }

    #[test]
    fn rows_from_other_days_do_not_fill_the_daily_meters() {
        let yesterday = (chrono::Utc::now().date_naive() - chrono::Duration::days(1)).to_string();
        let snapshot = payload_with_row(&format!("{yesterday}T12:00:00Z"), 812_400, 187_600)
            .snapshot()
            .expect("snapshot");
        assert!(snapshot.daily_input.is_none());
        assert!(snapshot.daily_output.is_none());
        assert_eq!(snapshot.daily_consumed_pct(), 0);
        // The per-model rollup still spans the whole settled window.
        assert_eq!(snapshot.models.len(), 1);
    }

    #[test]
    fn the_wallet_balance_can_go_negative() {
        let mut payload = payload_with_row("2026-01-01T00:00:00Z", 0, 0);
        payload.credits = Credits {
            total_credits: 10.0,
            total_usage: 12.5,
        };
        let snapshot = payload.snapshot().expect("snapshot");
        assert_eq!(snapshot.balance_usd, Some(-2.5));
        assert_eq!(snapshot.balance(), -2.5);
    }
}
