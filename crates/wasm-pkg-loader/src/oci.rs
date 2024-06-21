//! OCI package loader.
//!
//! This follows the CNCF TAG Runtime guidance for [Wasm OCI Artifacts][1].
//!
//! [1]: https://tag-runtime.cncf.io/wgs/wasm/deliverables/wasm-oci-artifact/

mod config;
pub(crate) mod source;

/// Re-exported for convenience.
pub use oci_distribution::client;

pub use config::{BasicCredentials, OciRegistryConfig};
