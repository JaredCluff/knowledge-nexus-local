//! Agent-native HTTP API (P9).
//!
//! Five endpoints under /v1/memory/*:
//! - POST /v1/memory/observe — write
//! - POST /v1/memory/recall — read (with token budget + optional federation)
//! - POST /v1/memory/reflect — on-demand reflection
//! - GET  /v1/memory/timeline — chronological event browse
//! - POST /v1/memory/forget — soft-archive (no hard delete via API)

pub mod bundler;
pub mod memory;

pub use bundler::{pack_to_budget, BundledResponse, BundledItem};
