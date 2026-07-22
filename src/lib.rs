//! ai-usagebar library — shared core for the Waybar widget and TUI binaries.
//!
//! The crate is organized by concern, not by binary:
//! - low-level primitives (`cache`, `countdown`, `pacing`, `pango`, `theme`)
//! - the vendor abstraction (`vendor`, `vendors::*`, `usage`)
//! - bin-specific composition (`widget`, `tui`) which lives next to its binary
//!
//! The two binaries (`ai-usagebar` and `ai-usagebar-tui`) are thin: they parse
//! CLI args, instantiate vendors, and hand off to a renderer in this crate.

pub mod active;
pub mod anthropic;
pub mod anthropic_api;
pub mod antigravity;
pub mod cache;
pub mod config;
pub mod context;
pub mod countdown;
pub mod deepseek;
pub mod error;
pub mod format;
pub mod grok;
pub mod kilo;
pub mod kimi;
pub mod moonshot;
pub mod novita;
pub mod openai;
pub mod openrouter;
pub mod pacing;
pub mod pango;
pub mod theme;
pub mod tooltip;
pub mod tui;
pub mod usage;
pub mod vendor;
pub mod waybar;
pub mod widget;
pub mod zai;

pub use error::{AppError, Result};
