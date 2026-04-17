//! Library surface for knowledge-nexus-agent.
//!
//! Keeps internal modules private to the binary while exposing the subset
//! that integration tests in `tests/` need.

pub mod config;
pub mod embeddings;
pub mod migrate;
pub mod store;
pub mod vectordb;
