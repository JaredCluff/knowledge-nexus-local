//! K2K (Knowledge-to-Knowledge) Federation Protocol Server
//!
//! Exposes local file semantic search via K2K protocol, allowing external AI agents
//! (ClawdBot, custom scripts, etc.) to query local knowledge with RSA-256 JWT authentication.

pub mod capabilities;
pub mod handlers;
pub mod keys;
pub mod middleware;
pub mod models;
pub mod remote_handler;
pub mod server;
pub mod task_handlers;
pub mod tasks;

pub use server::K2KServer;
