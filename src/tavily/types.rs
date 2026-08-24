//! Wire types for Tavily's documented `GET /usage` endpoint
//! (<https://docs.tavily.com/documentation/api-reference/endpoint/usage>).
//!
//! The response carries two blocks — `key` (this API key's usage) and
//! `account` (the subscription plan + pay-as-you-go + per-endpoint breakdown).
//! Every counter is an integer credit count; there is no money and no reset
//! timestamp in the documented schema, so the snapshot carries no currency and
//! no `UsageWindow`.

use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::usage::TavilySnapshot;

#[derive(Debug, Clone, Deserialize)]
pub struct UsageResponse {
    key: KeyUsage,
    account: AccountUsage,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KeyUsage {
    #[serde(rename = "usage")]
    used: u64,
    /// `null` when the key is unlimited.
    #[serde(rename = "limit", default)]
    limit: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccountUsage {
    #[serde(rename = "current_plan")]
    current_plan: String,
    #[serde(rename = "plan_usage")]
    plan_used: u64,
    /// `null` when the plan is unlimited.
    #[serde(rename = "plan_limit", default)]
    plan_limit: Option<u64>,
    #[serde(rename = "paygo_usage", default)]
    payg_used: Option<u64>,
    #[serde(rename = "paygo_limit", default)]
    payg_limit: Option<u64>,
    #[serde(rename = "search_usage", default)]
    search_usage: Option<u64>,
    #[serde(rename = "extract_usage", default)]
    extract_usage: Option<u64>,
    #[serde(rename = "crawl_usage", default)]
    crawl_usage: Option<u64>,
    #[serde(rename = "map_usage", default)]
    map_usage: Option<u64>,
    #[serde(rename = "research_usage", default)]
    research_usage: Option<u64>,
}

impl UsageResponse {
    pub fn into_snapshot(self, scope_fingerprint: String) -> Result<TavilySnapshot> {
        let account = self.account;
        let plan = account.current_plan.trim();
        if plan.is_empty() {
            return Err(AppError::Schema(
                "tavily: account.current_plan is empty".into(),
            ));
        }
        Ok(TavilySnapshot {
            plan: plan.to_string(),
            plan_used: account.plan_used,
            plan_limit: account.plan_limit,
            payg_used: account.payg_used.unwrap_or(0),
            payg_limit: account.payg_limit,
            key_used: self.key.used,
            key_limit: self.key.limit,
            search: account.search_usage.unwrap_or(0),
            extract: account.extract_usage.unwrap_or(0),
            crawl: account.crawl_usage.unwrap_or(0),
            map: account.map_usage.unwrap_or(0),
            research: account.research_usage.unwrap_or(0),
            scope_fingerprint,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "key": { "usage": 150, "limit": 1000,
                 "search_usage": 100, "extract_usage": 25,
                 "crawl_usage": 15, "map_usage": 7, "research_usage": 3 },
        "account": { "current_plan": "Bootstrap", "plan_usage": 500, "plan_limit": 15000,
                     "paygo_usage": 25, "paygo_limit": 100,
                     "search_usage": 350, "extract_usage": 75,
                     "crawl_usage": 50, "map_usage": 15, "research_usage": 10 }
    }"#;

    #[test]
    fn parses_the_documented_response_shape() {
        let snap = serde_json::from_str::<UsageResponse>(SAMPLE)
            .unwrap()
            .into_snapshot("fp".into())
            .unwrap();
        assert_eq!(snap.plan, "Bootstrap");
        assert_eq!(snap.plan_used, 500);
        assert_eq!(snap.plan_limit, Some(15000));
        assert_eq!(snap.payg_used, 25);
        assert_eq!(snap.payg_limit, Some(100));
        assert_eq!(snap.key_used, 150);
        assert_eq!(snap.key_limit, Some(1000));
        assert_eq!(snap.search, 350);
        assert_eq!(snap.extract, 75);
        assert_eq!(snap.crawl, 50);
        assert_eq!(snap.map, 15);
        assert_eq!(snap.research, 10);
        assert_eq!(snap.scope_fingerprint, "fp");
        assert_eq!(snap.plan_pct(), Some(3));
    }

    #[test]
    fn null_limits_mean_unlimited() {
        let raw = r#"{
            "key": { "usage": 10, "limit": null },
            "account": { "current_plan": "Free", "plan_usage": 5, "plan_limit": null }
        }"#;
        let snap = serde_json::from_str::<UsageResponse>(raw)
            .unwrap()
            .into_snapshot(String::new())
            .unwrap();
        assert_eq!(snap.key_limit, None);
        assert_eq!(snap.plan_limit, None);
        assert_eq!(snap.plan_pct(), None);
        assert_eq!(snap.payg_used, 0);
        assert_eq!(snap.payg_limit, None);
        assert_eq!(snap.search, 0);
    }

    #[test]
    fn empty_plan_name_is_schema_drift() {
        let raw = r#"{
            "key": { "usage": 0, "limit": 1000 },
            "account": { "current_plan": "  ", "plan_usage": 0, "plan_limit": 1000 }
        }"#;
        let err = serde_json::from_str::<UsageResponse>(raw)
            .unwrap()
            .into_snapshot(String::new())
            .unwrap_err();
        assert!(err.to_string().contains("current_plan"), "{err}");
    }

    #[test]
    fn missing_account_block_is_schema_drift() {
        let raw = r#"{
            "key": { "usage": 0, "limit": 1000 }
        }"#;
        let err = serde_json::from_str::<UsageResponse>(raw).unwrap_err();
        assert!(err.to_string().contains("account"), "{err}");
    }

    #[test]
    fn negative_counters_do_not_deserialize() {
        let raw = r#"{
            "key": { "usage": -1, "limit": 1000 },
            "account": { "current_plan": "Pro", "plan_usage": 0, "plan_limit": 1000 }
        }"#;
        assert!(serde_json::from_str::<UsageResponse>(raw).is_err());
    }
}
