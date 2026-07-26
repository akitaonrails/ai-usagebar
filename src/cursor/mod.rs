//! Cursor — premium-request quota read from the local Cursor IDE session
//! token (`state.vscdb`) against the undocumented `cursor.com/api/usage`
//! endpoint the Cursor dashboard itself calls. See `db.rs` for the token
//! source and `fetch.rs`/`types.rs` for the wire call and schema.

pub mod db;
pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
