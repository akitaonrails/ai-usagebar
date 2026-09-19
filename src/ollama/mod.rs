//! Ollama Cloud (`ollama.com`) — the *cloud* quota service behind the
//! "Cloud usage" section of `ollama.com/settings` and the session/weekly
//! limits enforced by remote models (`*-cloud` tags) in the official CLI.
//!
//! **Not the local daemon.** `http://127.0.0.1:11434` is unauthenticated and
//! has no quota route (`GET /api/usage` returns 404). Quota lives only on
//! `ollama.com`, gated by the Bearer keys minted at
//! `ollama.com/settings/keys`.
//!
//! Endpoint contract, live-verified 2026-09-09 against a real Pro account:
//!
//! ```json
//! GET https://ollama.com/api/usage  -> 200
//! {
//!   "limits": {
//!     "session": {
//!       "usage": 0.819,
//!       "models": [{"name":"kimi-k3","request_count":180}]
//!     },
//!     "weekly": {
//!       "usage": 0.23,
//!       "models": [{"name":"kimi-k3","request_count":180}]
//!     }
//!   },
//!   "activity": {
//!     "cost": "0.00000",
//!     "period": {"type":"last_4_weeks", "...": "..."},
//!     "models": []
//!   }
//! }
//! ```
//!
//! `usage` is a **fraction in [0, 1]**, not a percentage — `0.819` renders as
//! 82%. Model rows carry a request count per window. `activity.cost` arrives
//! as a **string of dollars** (`"0.00000"`), not a number. The payload does
//! not include a plan label or reset timestamps (those exist only in the
//! HTML UI).
//!
//! Auth: `Authorization: Bearer <key>` from `OLLAMA_API_KEY` (or config
//! `api_key`). The Ed25519 CLI session in `~/.ollama/id_ed25519` is a
//! **registry** credential and is refused here with 401.

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
