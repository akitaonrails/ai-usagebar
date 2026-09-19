//! The projected shape of a custom provider's response.
//!
//! Every built-in vendor has a `types.rs` that mirrors an upstream schema. A
//! custom provider has no upstream schema to mirror — the schema is whatever
//! the user pointed at — so what the rest of the program sees is this
//! already-projected snapshot. It is also what the cache stores: the raw body
//! is never persisted, so a cached payload cannot carry fields the user never
//! asked to display, and the fallback parse is a plain `serde_json` round
//! trip of this type.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomSnapshot {
    pub plan: Option<String>,
    pub metrics: Vec<CustomMetric>,
    pub texts: Vec<CustomText>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomMetric {
    pub label: String,
    /// `0..=100`, already rounded and clamped.
    pub pct: u16,
    /// `"12 of 100"` when the metric came from `used`/`limit`; `""` when the
    /// provider only reported a percentage.
    pub footnote: String,
    pub resets_at: Option<DateTime<Utc>>,
    pub window_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomText {
    pub label: String,
    pub value: String,
}
