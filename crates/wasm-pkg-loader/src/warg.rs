//! Warg package loader.
//!
//!

mod config;
pub(crate) mod source;

/// Re-exported for convenience.
pub use warg_client as client;

pub use config::WargRegistryConfig;
