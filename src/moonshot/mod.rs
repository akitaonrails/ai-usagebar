//! Moonshot / Kimi vendor — account balance from `/v1/users/me/balance` over
//! an API key. USD on `api.moonshot.ai`, CNY on `api.moonshot.cn`.

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
