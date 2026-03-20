//! Security module - Path whitelist and audit logging.
//!
//! SECURITY CRITICAL: This module enforces local file access controls
//! that CANNOT be modified by remote commands.

mod audit;
mod whitelist;

pub use whitelist::PathWhitelist;
