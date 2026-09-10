//! Wire types for the unofficial-but-stable `https://ollama.com/api/usage`
//! endpoint. Fields are `Option` / `Default` where the server may omit them
//! (a fresh account can have empty `models` or no `activity`).

use serde::Deserialize;

use crate::usage::{OllamaModelUsage, OllamaSnapshot, UsageWindow};

/// Top-level body of `GET /api/usage`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Body {
    #[serde(default)]
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

impl Body {
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
        let (cost, period_kind) = match self.activity {
            Some(a) => (a.cost, a.period.map(|p| p.kind)),
            None => (None, None),
        };

        OllamaSnapshot {
            plan,
            session: Self::window(self.limits.session, chrono::Duration::hours(5)),
            weekly: Self::window(self.limits.weekly, chrono::Duration::days(7)),
            session_models,
            weekly_models,
            activity_cost: cost,
            activity_period: period_kind,
        }
    }
}

#[cfg(test)]
mod tests {
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
        assert!(snap.session_models.is_empty());
        assert!(snap.weekly_models.is_empty());
    }

    #[test]
    fn clamps_over_full_and_negative_usage() {
        assert_eq!(Body::pct(Some(1.4)), 100);
        assert_eq!(Body::pct(Some(-0.2)), 0);
        assert_eq!(Body::pct(None), 0);
        assert_eq!(Body::pct(Some(0.5)), 50);
    }
}
