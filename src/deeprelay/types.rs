//! DeepRelay wire types and their projection into [`DeepRelaySnapshot`].
//!
//! One endpoint carries every plan figure — `GET /v1/billing/subscription`
//! (scope `billing:read`): the plan, four weighted-token meters, and the dollar
//! usage ceiling. `GET /v1/billing/balance` is the pay-as-you-go wallet and is
//! fetched only when it can matter.
//!
//! Two units need care:
//!
//! - Token figures are **weighted** tokens — a model with usage factor ×24
//!   draws 24 tokens of allowance per token served — so the meters are directly
//!   comparable with their limits and with each other.
//! - `*_micro_cents` is a millionth of a cent, so dollars = value / 1e8.
//!
//! The rolling 7-day caps have **no reset instant** by design ("It slides
//! rather than resets"), so `resets_at` stays `None` for them; never synthesize
//! a date. The monthly meters reset at the end of the billing period.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::usage::{DeepRelayMeter, DeepRelayModelUsage, DeepRelaySnapshot};

/// One micro-cent unit expressed as a fraction of a dollar.
const MICRO_CENTS_PER_DOLLAR: f64 = 100_000_000.0;

/// `GET /v1/billing/subscription` (`billing:read`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Subscription {
    /// Required on purpose: without it the body is not a subscription at all,
    /// and a 200 carrying something else must fail as a schema mismatch rather
    /// than project an all-empty snapshot.
    pub subscribed: bool,
    #[serde(default)]
    pub cancel_at_period_end: bool,
    #[serde(default)]
    pub current_period_start: Option<String>,
    #[serde(default)]
    pub current_period_end: Option<String>,
    #[serde(default)]
    pub plan: Option<Plan>,
    #[serde(default)]
    pub quota: Option<Quota>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Plan {
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub price: Option<Price>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Price {
    #[serde(default)]
    pub amount_cents: Option<i64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub interval: Option<String>,
    #[serde(default)]
    pub interval_count: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Quota {
    pub input_tokens_monthly: i64,
    pub output_tokens_monthly: i64,
    pub weekly_input_tokens: i64,
    pub weekly_output_tokens: i64,
    pub max_usage_micro_cents: i64,
    #[serde(default)]
    pub searches_monthly: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Usage {
    pub weighted_input_tokens: i64,
    pub weighted_output_tokens: i64,
    pub weekly_weighted_input_tokens: i64,
    pub weekly_weighted_output_tokens: i64,
    pub cost_micro_cents: i64,
    #[serde(default)]
    pub recent_payg_calls_24h: Option<i64>,
    #[serde(default)]
    pub searches: Option<i64>,
    #[serde(default)]
    pub by_model: Vec<ModelUsage>,
    #[serde(default)]
    pub weekly_by_model: Vec<ModelUsage>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ModelUsage {
    pub model: String,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub cached_input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub usage_factor_milli: Option<i64>,
    #[serde(default)]
    pub weighted_input_tokens: i64,
    #[serde(default)]
    pub weighted_output_tokens: i64,
}

/// `GET /v1/billing/balance` (`billing:read`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Balance {
    /// Cents are exact; `balance_dollars` is the same figure as a float and is
    /// deliberately not read, so two representations cannot disagree.
    pub balance_cents: i64,
    #[serde(default)]
    pub auto_pay_enabled: bool,
    #[serde(default)]
    pub auto_pay_threshold_cents: Option<i64>,
    #[serde(default)]
    pub burn_cents_per_hour: Option<i64>,
}

/// What the vendor cache stores: the subscription plus the wallet when the
/// second request was worth making. Keeping both in one payload means a cached
/// or stale read still carries the balance it was fetched with.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CachedPayload {
    pub subscription: Subscription,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<Balance>,
}

/// How many per-model rows the tooltip keeps. The response can carry dozens of
/// models; the figure that matters is the allowance, not the tail.
const MODEL_ROWS: usize = 5;

impl CachedPayload {
    pub fn snapshot(&self) -> Result<DeepRelaySnapshot> {
        let mut snapshot = self.subscription.snapshot()?;
        if let Some(balance) = &self.balance {
            snapshot.balance_usd = Some(cents_to_dollars(balance.balance_cents));
        }
        Ok(snapshot)
    }
}

impl Subscription {
    pub fn snapshot(&self) -> Result<DeepRelaySnapshot> {
        let period_start =
            parse_instant(self.current_period_start.as_deref(), "current_period_start")?;
        let period_end = parse_instant(self.current_period_end.as_deref(), "current_period_end")?;
        let usage = self.usage.as_ref();
        let quota = self.quota.as_ref();

        let meters = (|| -> Result<(Option<DeepRelayMeter>, Option<DeepRelayMeter>)> {
            let (Some(quota), Some(usage)) = (quota, usage) else {
                return Ok((None, None));
            };
            Ok((
                meter(usage.weighted_input_tokens, quota.input_tokens_monthly)?,
                meter(usage.weighted_output_tokens, quota.output_tokens_monthly)?,
            ))
        })();
        let (monthly_input, monthly_output) = meters?;

        let weekly = (|| -> Result<(Option<DeepRelayMeter>, Option<DeepRelayMeter>)> {
            let (Some(quota), Some(usage)) = (quota, usage) else {
                return Ok((None, None));
            };
            Ok((
                meter(
                    usage.weekly_weighted_input_tokens,
                    quota.weekly_input_tokens,
                )?,
                meter(
                    usage.weekly_weighted_output_tokens,
                    quota.weekly_output_tokens,
                )?,
            ))
        })();
        let (weekly_input, weekly_output) = weekly?;

        let (backstop_used_usd, backstop_limit_usd) = match (quota, usage) {
            (Some(quota), Some(usage)) if quota.max_usage_micro_cents > 0 => (
                Some(micro_cents_to_dollars(usage.cost_micro_cents)?),
                Some(micro_cents_to_dollars(quota.max_usage_micro_cents)?),
            ),
            _ => (None, None),
        };

        let searches = match (quota.and_then(|q| q.searches_monthly), usage) {
            (Some(limit), Some(usage)) if limit > 0 => {
                let used = non_negative(usage.searches.unwrap_or(0), "usage.searches")?;
                Some((used as u64, limit as u64))
            }
            _ => None,
        };

        let payg_calls_24h = usage
            .and_then(|usage| usage.recent_payg_calls_24h)
            .map(|calls| non_negative(calls, "usage.recent_payg_calls_24h"))
            .transpose()?
            .map(|calls| calls as u64);

        let plan = match (
            self.subscribed,
            self.plan.as_ref().and_then(|p| p.name.clone()),
        ) {
            (true, Some(name)) => name,
            (true, None) => "DeepRelay".to_string(),
            (false, _) => "Pay as you go".to_string(),
        };
        let price = self.plan.as_ref().and_then(|plan| plan.price.as_ref());
        let plan_price_cents = price.and_then(|price| price.amount_cents);
        let plan_interval = match price {
            Some(price) => price
                .interval
                .clone()
                .map(|interval| match price.interval_count {
                    Some(count) if count > 1 => format!("{count} {interval}s"),
                    _ => interval,
                }),
            None => None,
        };

        Ok(DeepRelaySnapshot {
            plan,
            plan_price_cents,
            plan_interval,
            subscribed: self.subscribed,
            cancel_at_period_end: self.cancel_at_period_end,
            monthly_input,
            monthly_output,
            weekly_input,
            weekly_output,
            backstop_used_usd,
            backstop_limit_usd,
            searches,
            payg_calls_24h,
            balance_usd: None,
            period_start,
            period_end,
            models: top_models(usage),
        })
    }
}

/// A meter exists only when the plan states a ceiling. An omitted or zero limit
/// is "uncapped", not "0% used": `used = 0, limit = 0` is not a reading.
fn meter(used: i64, limit: i64) -> Result<Option<DeepRelayMeter>> {
    let used = non_negative(used, "weighted tokens")?;
    let limit = non_negative(limit, "token allowance")?;
    if limit == 0 {
        return Ok(None);
    }
    Ok(Some(DeepRelayMeter { used, limit }))
}

fn non_negative(value: i64, field: &str) -> Result<i64> {
    if value < 0 {
        return Err(AppError::Schema(format!("DeepRelay {field} is negative")));
    }
    Ok(value)
}

fn micro_cents_to_dollars(value: i64) -> Result<f64> {
    Ok(non_negative(value, "cost")? as f64 / MICRO_CENTS_PER_DOLLAR)
}

/// A balance can legitimately be negative if an overrun was recorded, so this
/// deliberately does not reject the sign.
fn cents_to_dollars(value: i64) -> f64 {
    value as f64 / 100.0
}

fn parse_instant(raw: Option<&str>, field: &str) -> Result<Option<DateTime<Utc>>> {
    raw.map(|raw| {
        DateTime::parse_from_rfc3339(raw)
            .map(|when| when.with_timezone(&Utc))
            .map_err(|_| AppError::Schema(format!("DeepRelay {field} is not an RFC 3339 instant")))
    })
    .transpose()
}

/// The heaviest models of the billing period, for the tooltip. The wire ids
/// carry a `deeprelay/` namespace that is noise next to the plan's own label.
fn top_models(usage: Option<&Usage>) -> Vec<DeepRelayModelUsage> {
    let Some(usage) = usage else {
        return Vec::new();
    };
    let mut models: Vec<DeepRelayModelUsage> = usage
        .by_model
        .iter()
        .map(|model| DeepRelayModelUsage {
            model: model
                .model
                .strip_prefix("deeprelay/")
                .unwrap_or(&model.model)
                .to_string(),
            weighted_input_tokens: model.weighted_input_tokens.max(0),
            weighted_output_tokens: model.weighted_output_tokens.max(0),
        })
        .filter(|model| {
            model
                .weighted_input_tokens
                .saturating_add(model.weighted_output_tokens)
                > 0
        })
        .collect();
    models.sort_by(|a, b| {
        let a_total = a
            .weighted_input_tokens
            .saturating_add(a.weighted_output_tokens);
        let b_total = b
            .weighted_input_tokens
            .saturating_add(b.weighted_output_tokens);
        b_total.cmp(&a_total)
    });
    models.truncate(MODEL_ROWS);
    models
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = include_str!("../../tests/fixtures/deeprelay/subscription.json");

    fn parsed(raw: &str) -> DeepRelaySnapshot {
        serde_json::from_str::<Subscription>(raw)
            .expect("wire shape")
            .snapshot()
            .expect("snapshot")
    }

    #[test]
    fn full_response_projects_every_meter() {
        let snapshot = parsed(FULL);
        assert_eq!(snapshot.plan, "Plus");
        assert_eq!(snapshot.plan_price_cents, Some(499));
        assert_eq!(snapshot.plan_interval.as_deref(), Some("month"));
        assert!(snapshot.subscribed);
        assert!(!snapshot.cancel_at_period_end);

        let monthly_in = snapshot.monthly_input.expect("monthly input meter");
        assert_eq!(monthly_in.used, 812_400_000);
        assert_eq!(monthly_in.limit, 2_500_000_000);
        assert_eq!(monthly_in.pct(), Some(32));

        let weekly_in = snapshot.weekly_input.expect("weekly input meter");
        assert_eq!(weekly_in.pct(), Some(50));
        // Rolling caps have no reset instant.
        assert_eq!(snapshot.weekly_window().expect("window").resets_at, None);
        assert_eq!(
            snapshot.monthly_window().expect("window").resets_at,
            snapshot.period_end
        );

        assert_eq!(snapshot.backstop_used_usd, Some(12.34));
        assert_eq!(snapshot.backstop_limit_usd, Some(500.0));
        assert_eq!(snapshot.searches, Some((7, 500)));
        assert_eq!(snapshot.payg_calls_24h, Some(0));
        assert_eq!(snapshot.models[0].model, "deepseek-v4.1-flash");
        assert_eq!(snapshot.models.len(), 3);
    }

    #[test]
    fn pay_as_you_go_accounts_have_no_meters() {
        let snapshot = parsed(
            r#"{"subscribed":false,"cancel_at_period_end":false,"plan":null,
                "quota":null,"usage":null}"#,
        );
        assert_eq!(snapshot.plan, "Pay as you go");
        assert!(snapshot.monthly_input.is_none());
        assert!(snapshot.weekly_window().is_none());
        assert!(snapshot.monthly_window().is_none());
    }

    #[test]
    fn an_incomplete_usage_object_is_rejected_instead_of_fabricating_zero() {
        let error = serde_json::from_str::<Subscription>(
            r#"{"subscribed":true,"plan":{"name":"Base"},
                "quota":{"input_tokens_monthly":500000000,"output_tokens_monthly":50000000,
                         "weekly_input_tokens":125000000,"weekly_output_tokens":12500000,
                         "max_usage_micro_cents":10000000000},
                "usage":{"weighted_input_tokens":500000000,"weighted_output_tokens":0}}"#,
        )
        .expect_err("missing usage meters are unknown, not zero");
        assert!(error.to_string().contains("weekly_weighted_input_tokens"));
    }

    #[test]
    fn zero_ceiling_is_uncapped_and_negative_figures_are_rejected() {
        let snapshot = parsed(
            r#"{"subscribed":true,"plan":{"name":"Custom"},
                "quota":{"input_tokens_monthly":0,"output_tokens_monthly":0,
                         "weekly_input_tokens":0,"weekly_output_tokens":0,
                         "max_usage_micro_cents":0},
                "usage":{"weighted_input_tokens":0,"weighted_output_tokens":0,
                         "weekly_weighted_input_tokens":0,"weekly_weighted_output_tokens":0,
                         "cost_micro_cents":0}}"#,
        );
        assert!(snapshot.monthly_input.is_none());
        assert_eq!(snapshot.backstop_limit_usd, None);

        let error = serde_json::from_str::<Subscription>(
            r#"{"subscribed":true,"quota":{"input_tokens_monthly":10,"output_tokens_monthly":10,
                "weekly_input_tokens":10,"weekly_output_tokens":10,
                "max_usage_micro_cents":10},"usage":{"weighted_input_tokens":-1,
                "weighted_output_tokens":0,"weekly_weighted_input_tokens":0,
                "weekly_weighted_output_tokens":0,"cost_micro_cents":0}}"#,
        )
        .unwrap()
        .snapshot()
        .expect_err("negative usage is invalid wire data");
        assert!(matches!(error, AppError::Schema(_)));
    }

    #[test]
    fn a_body_that_is_not_a_subscription_is_a_schema_mismatch() {
        assert!(serde_json::from_str::<Subscription>(r#"{"error":"unauthorized"}"#).is_err());
    }

    #[test]
    fn a_malformed_period_instant_is_rejected() {
        let subscription: Subscription = serde_json::from_str(
            r#"{"subscribed":false,"current_period_end":"tomorrow","usage":null}"#,
        )
        .unwrap();
        let error = subscription.snapshot().expect_err("invalid period instant");
        assert!(matches!(error, AppError::Schema(_)));
        assert!(error.to_string().contains("current_period_end"));
    }

    #[test]
    fn model_totals_saturate_at_the_wire_integer_limit() {
        let usage = Usage {
            by_model: vec![ModelUsage {
                model: "deeprelay/large".into(),
                weighted_input_tokens: i64::MAX,
                weighted_output_tokens: i64::MAX,
                ..Default::default()
            }],
            ..Default::default()
        };
        let models = top_models(Some(&usage));
        assert_eq!(models[0].weighted_total(), i64::MAX);
    }

    #[test]
    fn balance_is_read_in_cents_and_attached_to_the_snapshot() {
        let payload: CachedPayload = serde_json::from_str(
            r#"{"subscription":{"subscribed":false},
                "balance":{"balance_cents":1234,"auto_pay_enabled":false}}"#,
        )
        .unwrap();
        assert_eq!(payload.snapshot().unwrap().balance_usd, Some(12.34));
    }
}
