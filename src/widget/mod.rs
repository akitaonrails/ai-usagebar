//! Widget binary support: CLI parsing, per-vendor rendering, the always-exit-0
//! wrapper, and the local-testing renderers (`--pretty`, `--watch`, `--json`).
//!
//! The library exposes building blocks here; the actual `main` lives in
//! `src/bin/ai-usagebar.rs`, which is a thin orchestration layer.

pub mod cli;
pub mod pretty;
pub mod render;
pub mod run;
