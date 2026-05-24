//! OpenAI vendor — Codex OAuth via `~/.codex/auth.json` + the undocumented
//! `chatgpt.com/backend-api/wham/usage` endpoint. Reference:
//! `~/Projects/codexbar/codexbar` by the same author as claudebar.

pub mod creds;
pub mod fetch;
pub mod oauth;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
pub use vendor::OpenAiVendor;
