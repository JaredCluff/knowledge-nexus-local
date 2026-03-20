//! WebSocket connection to Agent Hub.
//!
//! Maintains persistent connection with automatic reconnection.

mod client;
mod messages;

pub use client::start_connection;
