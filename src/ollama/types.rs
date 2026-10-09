//! Wire types for `https://ollama.com/api/balance` and historical quota
//! payloads cached from `/api/usage`. Current usage-history responses are
//! deliberately rejected: request/token totals are not quota limits.

use serde::Deserialize;

use crate::usage::{OllamaCredits, OllamaModelUsage, OllamaSnapshot, UsageWindow};

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Body {
    Balance(BalanceBody),
    Legacy(LegacyBody),
}

impl Body {
    pub fn into_snapshot(self, plan: String) -> OllamaSnapshot {
        match self {
            Self::Balance(body) => body.into_snapshot(plan),
            Self::Legacy(body) => body.into_snapshot(plan),
        }
    }
}

/// Required discriminator prevents unrelated JSON from becoming empty quotas.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LegacyBody {
    pub limits: Limits,
    #[serde(default)]
    pub activity: Option<Activity>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Limits {
    #[serde(default)]
    pub session: Option<Window>,
    #[serde(default)]
    pub weekly: Option<Window>,
    /// Calendar-month quota (`limits.monthly`). Reported instead of
    /// `session`/`weekly` on at least some Pro accounts — the two shapes are
    /// mutually observed, never combined in one response so far.
    #[serde(default)]
    pub monthly: Option<Window>,
}

/// One quota window. `usage` is a fraction in `[0.0, 1.0]`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Window {
    #[serde(default)]
    pub usage: Option<f64>,
    #[serde(default)]
    pub models: Vec<ModelUsage>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ModelUsage {
    pub name: String,
    #[serde(default)]
    pub request_count: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Activity {
    /// Dollars as a string, e.g. `"0.00000"`. Kept exact so a tooltip can
    /// show what the server sent without rounding drift.
    #[serde(default)]
    pub cost: Option<String>,
    #[serde(default)]
    pub period: Option<Period>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Period {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub starting_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub ending_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl LegacyBody {
    /// Fraction in `[0, 1]` → percent `0..=100`, saturated. Missing usage is 0%.
    fn pct(frac: Option<f64>) -> i32 {
        let f = frac.unwrap_or(0.0).clamp(0.0, 1.0);
        (f * 100.0).round() as i32
    }

    fn models(w: Option<&Window>) -> Vec<OllamaModelUsage> {
        w.map(|window| {
            window
                .models
                .iter()
                .map(|m| OllamaModelUsage {
                    name: m.name.clone(),
                    request_count: m.request_count,
                })
                .collect()
        })
        .unwrap_or_default()
    }

    fn window(w: Option<Window>, duration: chrono::Duration) -> Option<UsageWindow> {
        let w = w?;
        Some(UsageWindow {
            utilization_pct: Self::pct(w.usage),
            // The JSON payload does not carry a reset timestamp (the HTML UI
            // does). Renderers pace against the window length only.
            resets_at: None,
            window_duration: duration,
        })
    }

    /// Project the wire payload into the cacheable snapshot. `plan` comes from
    /// config — the server does not send a plan field on this route.
    pub fn into_snapshot(self, plan: String) -> OllamaSnapshot {
        let session_models = Self::models(self.limits.session.as_ref());
        let weekly_models = Self::models(self.limits.weekly.as_ref());
        let monthly_models = Self::models(self.limits.monthly.as_ref());
        let (cost, period_kind) = match self.activity {
            Some(a) => (a.cost, a.period.map(|p| p.kind)),
            None => (None, None),
        };

        OllamaSnapshot {
            plan,
            session: Self::window(self.limits.session, chrono::Duration::hours(5)),
            weekly: Self::window(self.limits.weekly, chrono::Duration::days(7)),
            // Nominal length only — the API gives no cycle-start date, so
            // pacing against a real subscription month is not possible. Kept
            // consistent with session/weekly, which also carry no reset time.
            monthly: Self::window(self.limits.monthly, chrono::Duration::days(30)),
            session_models,
            weekly_models,
            monthly_models,
            activity_cost: cost,
            activity_period: period_kind,
            credits: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct BalanceBody {
    pub included: Included,
    #[serde(default)]
    pub purchased: Option<Purchased>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Included {
    Credits {
        balance_usd: f64,
        allowance_usd: f64,
        period: BillingPeriod,
    },
    Legacy {
        session: RemainingWindow,
        weekly: RemainingWindow,
    },
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct BillingPeriod {
    pub from: chrono::DateTime<chrono::Utc>,
    pub until: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Purchased {
    pub balance_usd: f64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RemainingWindow {
    pub remaining_percent: f64,
    pub resets_at: chrono::DateTime<chrono::Utc>,
}

impl RemainingWindow {
    fn into_window(self, duration: chrono::Duration) -> UsageWindow {
        UsageWindow {
            utilization_pct: (100.0 - self.remaining_percent).clamp(0.0, 100.0).round() as i32,
            resets_at: Some(self.resets_at),
            window_duration: duration,
        }
    }
}

impl BalanceBody {
    fn into_snapshot(self, plan: String) -> OllamaSnapshot {
        let mut snap = LegacyBody {
            limits: Limits::default(),
            activity: None,
        }
        .into_snapshot(plan);
        match self.included {
            Included::Credits {
                balance_usd,
                allowance_usd,
                period,
            } => {
                // No measurable percentage for a zero allowance; still show
                // the actual dollar balance, without dividing by zero.
                if allowance_usd > 0.0 {
                    snap.monthly = Some(UsageWindow {
                        utilization_pct: LegacyBody::pct(Some(1.0 - balance_usd / allowance_usd)),
                        resets_at: Some(period.until),
                        window_duration: period.until - period.from,
                    });
                }
                snap.credits = Some(OllamaCredits {
                    balance: crate::format::usd(balance_usd),
                    allowance: crate::format::usd(allowance_usd),
                    purchased: self.purchased.map(|p| crate::format::usd(p.balance_usd)),
                });
            }
            Included::Legacy { session, weekly } => {
                snap.session = Some(session.into_window(chrono::Duration::hours(5)));
                snap.weekly = Some(weekly.into_window(chrono::Duration::days(7)));
            }
        }
        snap
    }
}

#[cfg(test)]
mod tests {
    /// A real `/api/balance` payload from a session/weekly account, captured by
    /// a third party on #387. The credits branch of `Included` had a committed
    /// capture; this one did not, and it is the branch every pre-existing user
    /// depends on — so the shape is pinned by the bytes the API actually sent
    /// rather than by a hand-written sample.
    #[test]
    fn a_real_quota_account_balance_payload_keeps_its_windows() {
        let raw = include_str!("../../tests/fixtures/ollama/balance_quota.json");
        let body: Body = serde_json::from_str(raw).expect("captured payload must parse");
        let snap = body.into_snapshot("pro".into());

        // `remaining_percent` is what is LEFT, so utilization is its complement:
        // 100 remaining is 0 used, and 59.19 remaining rounds to 41 used.
        let session = snap.session.expect("session window");
        assert_eq!(session.utilization_pct, 0);
        let weekly = snap.weekly.expect("weekly window");
        assert_eq!(weekly.utilization_pct, 41);

        // A quota account has no credits block and no monthly window; a zero
        // purchased balance must not invent one.
        assert!(
            snap.monthly.is_none(),
            "quota accounts report no monthly window"
        );
        assert!(snap.credits.is_none(), "a quota account has no credits");

        // The resets travel with the windows rather than being guessed.
        assert!(
            session.resets_at.is_some(),
            "session reset comes from the payload"
        );
        assert!(
            weekly.resets_at.is_some(),
            "weekly reset comes from the payload"
        );
    }

    use super::*;

    /// Real 200 body captured 2026-09-09 against a Pro account (numbers
    /// redacted only by rounding — structure is verbatim).
    const LIVE: &str = r#"{
      "activity": {
        "cost": "0.00000",
        "period": {
          "type": "last_4_weeks",
          "starting_at": "2026-08-17T00:00:00Z",
          "ending_at": "2026-09-09T18:28:57.120401373Z"
        },
        "models": []
      },
      "limits": {
        "session": {
          "usage": 0.819,
          "models": [
            {"name": "kimi-k3", "request_count": 180},
            {"name": "deepseek-v4-flash:0731", "request_count": 27},
            {"name": "minimax-m3", "request_count": 8},
            {"name": "glm-5.3-flash", "request_count": 8},
            {"name": "gpt-oss:120b", "request_count": 2}
          ]
        },
        "weekly": {
          "usage": 0.23,
          "models": [
            {"name": "kimi-k3", "request_count": 180},
            {"name": "minimax-m3", "request_count": 554},
            {"name": "deepseek-v4-flash:0731", "request_count": 27},
            {"name": "qwen3.5:397b", "request_count": 2},
            {"name": "glm-5.3-flash", "request_count": 8},
            {"name": "gpt-oss:120b", "request_count": 2}
          ]
        }
      }
    }"#;

    #[test]
    fn parses_live_captured_body() {
        let body: Body = serde_json::from_str(LIVE).unwrap();
        let snap = body.into_snapshot("pro".into());
        assert_eq!(snap.plan, "pro");
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 82);
        assert_eq!(snap.session_models[0].name, "kimi-k3");
        assert_eq!(snap.session_models[0].request_count, 180);
        assert_eq!(snap.session_models.len(), 5);
        assert_eq!(snap.weekly.as_ref().unwrap().utilization_pct, 23);
        assert_eq!(snap.weekly_models[1].name, "minimax-m3");
        assert_eq!(snap.weekly_models[1].request_count, 554);
        assert_eq!(snap.activity_cost.as_deref(), Some("0.00000"));
        assert_eq!(snap.activity_period.as_deref(), Some("last_4_weeks"));
    }

    #[test]
    fn missing_windows_are_none() {
        let body: Body = serde_json::from_str(r#"{"limits":{}}"#).unwrap();
        let snap = body.into_snapshot("free".into());
        assert!(snap.session.is_none());
        assert!(snap.weekly.is_none());
        assert!(snap.monthly.is_none());
        assert!(snap.session_models.is_empty());
        assert!(snap.weekly_models.is_empty());
        assert!(snap.monthly_models.is_empty());
    }

    /// Real 200 body captured 2026-09-16 against a different Pro account —
    /// this shape reports `limits.monthly` instead of `session`/`weekly`.
    /// Both shapes exist in the wild for the same "pro" plan label.
    const LIVE_MONTHLY: &str = r#"{
      "activity": {
        "cost": "0.00000",
        "period": {
          "type": "last_4_weeks",
          "starting_at": "2026-08-24T00:00:00Z",
          "ending_at": "2026-09-16T08:55:34.663902649Z"
        }
      },
      "limits": {
        "monthly": {
          "usage": 0.003,
          "models": [
            {"name": "gpt-oss:120b", "request_count": 100},
            {"name": "gpt-oss:20b", "request_count": 2}
          ]
        }
      }
    }"#;

    #[test]
    fn parses_live_captured_monthly_body() {
        let body: Body = serde_json::from_str(LIVE_MONTHLY).unwrap();
        let snap = body.into_snapshot("pro".into());
        assert!(snap.session.is_none());
        assert!(snap.weekly.is_none());
        assert_eq!(snap.monthly.as_ref().unwrap().utilization_pct, 0);
        assert_eq!(snap.monthly_models[0].name, "gpt-oss:120b");
        assert_eq!(snap.monthly_models[0].request_count, 100);
        assert_eq!(snap.monthly_models.len(), 2);
        assert_eq!(snap.activity_cost.as_deref(), Some("0.00000"));
    }

    #[test]
    fn credit_balance_projects_monthly_usage_and_exact_billing_period() {
        let body: Body = serde_json::from_str(include_str!(
            "../../tests/fixtures/ollama/balance_credits.json"
        ))
        .unwrap();
        let snap = body.into_snapshot("pro".into());
        let monthly = snap.monthly.unwrap();
        assert_eq!(monthly.utilization_pct, 25);
        assert_eq!(
            monthly.resets_at.unwrap().to_rfc3339(),
            "2026-11-08T08:00:00+00:00"
        );
        assert_eq!(monthly.window_duration, chrono::Duration::days(31));
        assert!(snap.session.is_none());
        assert!(snap.weekly.is_none());
        let credits = snap.credits.unwrap();
        assert_eq!(credits.balance, "$45.00");
        assert_eq!(credits.allowance, "$60.00");
        assert_eq!(credits.purchased.as_deref(), Some("$12.50"));
        assert!(
            snap.activity_cost.is_none(),
            "balance is not usage-history cost"
        );
    }

    #[test]
    fn legacy_balance_inverts_remaining_percent_and_keeps_resets() {
        let body: Body = serde_json::from_str(
            r#"{
          "included": {
            "session": {"remaining_percent":75,"resets_at":"2026-10-01T07:00:00Z"},
            "weekly": {"remaining_percent":40,"resets_at":"2026-10-05T00:00:00Z"}
          },
          "purchased": {"balance_usd":25}
        }"#,
        )
        .unwrap();
        let snap = body.into_snapshot("pro".into());
        let session = snap.session.unwrap();
        assert_eq!(session.utilization_pct, 25);
        assert_eq!(session.window_duration, chrono::Duration::hours(5));
        assert_eq!(
            session.resets_at.unwrap().to_rfc3339(),
            "2026-10-01T07:00:00+00:00"
        );
        assert_eq!(snap.weekly.unwrap().utilization_pct, 60);
        assert!(snap.monthly.is_none());
        assert!(snap.credits.is_none());
    }

    #[test]
    fn zero_allowance_is_not_a_fabricated_percentage() {
        let body: Body = serde_json::from_str(
            r#"{
          "included": {"balance_usd":0,"allowance_usd":0,
            "period":{"from":"2026-10-01T00:00:00Z","until":"2026-11-01T00:00:00Z"}}
        }"#,
        )
        .unwrap();
        let snap = body.into_snapshot("free".into());
        assert!(snap.monthly.is_none());
        assert_eq!(snap.credits.unwrap().balance, "$0.00");
    }

    #[test]
    fn usage_history_and_unknown_shapes_are_rejected() {
        for payload in [
            r#"{"range":"7d","totals":{"request_count":213,"usage_usd":0.2},"buckets":[]}"#,
            r#"{}"#,
            r#"{"included":{}}"#,
            r#"{"included":{"balance_usd":45}}"#,
        ] {
            assert!(serde_json::from_str::<Body>(payload).is_err(), "{payload}");
        }
    }

    #[test]
    fn clamps_over_full_and_negative_usage() {
        assert_eq!(LegacyBody::pct(Some(1.4)), 100);
        assert_eq!(LegacyBody::pct(Some(-0.2)), 0);
        assert_eq!(LegacyBody::pct(None), 0);
        assert_eq!(LegacyBody::pct(Some(0.5)), 50);
    }
}
