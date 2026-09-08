//! ai-usagebar library — shared core for the Waybar widget and TUI binaries.
//!
//! The crate is organized by concern, not by binary:
//! - low-level primitives (`cache`, `countdown`, `pacing`, `pango`, `theme`)
//! - the vendor abstraction (`vendor`, `vendors::*`, `usage`)
//! - bin-specific composition (`widget`, `tui`) which lives next to its binary
//!
//! The binaries (`ai-usagebar`, `ai-usagebar-tui`, and on Windows
//! `ai-usagebar-tray`) are thin: they parse CLI args, instantiate vendors,
//! and hand off to a renderer in this crate.

pub mod account;
pub mod active;
pub mod anthropic;
pub mod anthropic_api;
pub mod antigravity;
pub mod cache;
pub mod catalog;
pub mod claude_desktop;
pub mod commandcode;
pub mod config;
pub mod context;
pub mod copilot;
pub mod countdown;
pub mod cursor;
pub mod custom;
pub mod deepseek;
pub mod detect;
pub mod display;
pub mod error;
pub mod format;
pub mod grok;
/// Source-scanning helpers for structural guard tests. Test-only.
#[cfg(test)]
pub(crate) mod guard;
pub mod jwt;
pub mod kilo;
pub mod kimi;
pub mod kiro;
pub mod minimax;
pub mod moonshot;
pub mod nous;
pub mod novita;
pub mod openai;
pub mod opencode_go;
pub mod openrouter;
pub mod outcome;
pub mod pacing;
pub mod pango;
pub mod process;
pub mod report;
pub mod safe_storage;
pub mod serde_helpers;
pub mod supergrok;
pub mod theme;
pub mod tooltip;
pub mod tray;
pub mod tui;
pub mod update;
pub mod usage;
pub mod vendor;
pub mod waybar;
pub mod widget;
pub mod zai;

pub use error::{AppError, Result};
