//! Ollama Cloud (`ollama.com`) — the *cloud* quota service behind the
//! "Cloud usage" section of `ollama.com/settings` and the session/weekly
//! limits enforced by remote models (`*-cloud` tags) in the official CLI.
//!
//! **Not the local daemon.** `http://127.0.0.1:11434` is unauthenticated and
//! has no quota route. Quota lives only on `ollama.com`, gated by the Bearer
//! keys minted at `ollama.com/settings/keys`.
//!
//! Endpoint contract: the documented `GET https://ollama.com/api/balance`
//! (<https://docs.ollama.com/api/balance>). One request answers in one of two
//! shapes of `included`, told apart by their fields (`types::Included`);
//! `purchased` is optional in both.
//!
//! Credit-based plans (`tests/fixtures/ollama/balance_credits.json`):
//!
//! ```json
//! {
//!   "included": {
//!     "balance_usd": 45,
//!     "allowance_usd": 60,
//!     "period": {"from": "2026-10-08T08:00:00Z", "until": "2026-11-08T08:00:00Z"}
//!   },
//!   "purchased": {"balance_usd": 12.5}
//! }
//! ```
//!
//! `balance_usd` is what is **left** of `allowance_usd`, so monthly
//! utilization is `1 - balance / allowance` (25% here); a zero allowance
//! yields no percentage window. The reset is `period.until`, and the window
//! length is the period itself rather than a nominal 30 days.
//! `purchased.balance_usd` is reported beside the included credits and does
//! not enter the percentage.
//!
//! Session/weekly plans (`tests/fixtures/ollama/balance_quota.json`):
//!
//! ```json
//! {
//!   "included": {
//!     "session": {"remaining_percent": 100, "resets_at": "2026-10-08T19:00:00Z"},
//!     "weekly": {"remaining_percent": 59.19, "resets_at": "2026-10-12T00:00:00Z"}
//!   },
//!   "purchased": {"balance_usd": 0}
//! }
//! ```
//!
//! `remaining_percent` is a **percentage of what is left**, so utilization is
//! its complement — `59.19` renders as 41% used. Each window carries its own
//! `resets_at`. This shape projects no credits, so its `purchased` balance is
//! not shown.
//!
//! Neither shape includes a plan label; it comes from `[ollama] plan`.
//!
//! Historical: before `/api/balance` the quota was read from `/api/usage`,
//! live-verified 2026-09-09 against a real Pro account (the local daemon
//! answered the same path with 404):
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
//! HTML UI). Some accounts reported `limits.monthly` in place of the other
//! two windows.
//!
//! That route is no longer requested. Its quota payloads stay parseable
//! (`types::LegacyBody`) so a cache written by an older version still serves
//! as a fallback. `/api/usage` now answers with request/token/cost history
//! instead of quota; that shape is rejected, and a cache holding it is
//! replaced by a live balance fetch even while fresh.
//!
//! Auth: `Authorization: Bearer <key>` from `OLLAMA_API_KEY` (or config
//! `api_key`). The Ed25519 CLI session in `~/.ollama/id_ed25519` is a
//! **registry** credential (`pull` / `push`), not an API key, and is never
//! read (`/api/usage` refused it with 401).

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
