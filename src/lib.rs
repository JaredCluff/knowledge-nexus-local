//! Library surface for knowledge-nexus-agent.
//!
//! Keeps internal modules private to the binary while exposing the subset
//! that integration tests in `tests/` need.

pub mod api;
pub mod config;
pub mod embeddings;
pub mod maintenance;
pub mod policy;
/// Partial k2k re-export for lib-visible retrieval module.
/// The full k2k module (server, handlers, etc.) lives in main.rs because it
/// transitively depends on bin-only modules (connectors, federation, ...).
pub mod k2k {
    pub mod models;
}
pub mod knowledge;
pub mod migrate;
pub mod retrieval;
pub mod store;
pub mod vectordb;
