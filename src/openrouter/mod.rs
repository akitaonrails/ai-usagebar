//! OpenRouter vendor — `/api/v1/credits` and `/api/v1/key` over an API key.

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
pub use vendor::OpenRouterVendor;
