//! Wasm Package Client
//!
//! [`Client`] implements a unified interface for loading package content from
//! multiple kinds of package registries.
//!
//! # Example
//!
//! ```no_run
//! # async fn example() -> anyhow::Result<()> {
//! // Initialize client from global configuration.
//! let mut client = wasm_pkg_client::Client::with_global_defaults()?;
//!
//! // Get a specific package release version.
//! let pkg = "example:pkg".parse()?;
//! let version = "1.0.0".parse()?;
//! let release = client.get_release(&pkg, &version).await?;
//!
//! // Stream release content to a file.
//! let mut stream = client.stream_content(&pkg, &release).await?;
//! let mut file = tokio::fs::File::create("output.wasm").await?;
//! use futures_util::TryStreamExt;
//! use tokio::io::AsyncWriteExt;
//! while let Some(chunk) = stream.try_next().await? {
//!     file.write_all(&chunk).await?;
//! }
//! # Ok(()) }
//! ```

mod loader;
mod local;
pub mod oci;
pub mod warg;

use std::collections::HashMap;

use anyhow::anyhow;
use bytes::Bytes;
use futures_util::stream::BoxStream;

use wasm_pkg_common::metadata::RegistryMetadata;

use crate::{
    loader::PackageLoader, loader::VersionInfo, local::LocalBackend, oci::OciBackend,
    warg::WargBackend,
};

pub use wasm_pkg_common::{
    config::Config,
    digest::ContentDigest,
    package::{PackageRef, Version},
    registry::Registry,
    Error,
};

/// A read-only registry client.
pub struct Client {
    config: Config,
    sources: HashMap<Registry, Box<dyn PackageLoader>>,
}

impl Client {
    /// Returns a new client with the given [`Config`].
    pub fn new(config: Config) -> Self {
        Self {
            config,
            sources: Default::default(),
        }
    }

    /// Returns a new client configured from default global config.
    pub fn with_global_defaults() -> Result<Self, Error> {
        let config = Config::global_defaults()?;
        Ok(Self::new(config))
    }

    /// Returns a list of all package [`Version`]s available for the given package.
    pub async fn list_all_versions(
        &mut self,
        package: &PackageRef,
    ) -> Result<Vec<VersionInfo>, Error> {
        let source = self.resolve_source(package).await?;
        source.list_all_versions(package).await
    }

    /// Returns a [`Release`] for the given package version.
    pub async fn get_release(
        &mut self,
        package: &PackageRef,
        version: &Version,
    ) -> Result<Release, Error> {
        let source = self.resolve_source(package).await?;
        source.get_release(package, version).await
    }

    /// Returns a [`BoxStream`] of content chunks. Contents are validated
    /// against the given [`Release::content_digest`].
    pub async fn stream_content(
        &mut self,
        package: &PackageRef,
        release: &Release,
    ) -> Result<BoxStream<Result<Bytes, Error>>, Error> {
        let source = self.resolve_source(package).await?;
        source.stream_content(package, release).await
    }

    async fn resolve_source(
        &mut self,
        package: &PackageRef,
    ) -> Result<&mut dyn PackageLoader, Error> {
        let registry = self
            .config
            .resolve_registry(package)
            .ok_or_else(|| Error::NoRegistryForNamespace(package.namespace().clone()))?
            .to_owned();
        if !self.sources.contains_key(&registry) {
            let registry_config = self
                .config
                .registry_config(&registry)
                .cloned()
                .unwrap_or_default();

            // Skip fetching metadata for "local" source
            let should_fetch_meta = registry_config.backend_type() != Some("local");
            let registry_meta = if should_fetch_meta {
                RegistryMetadata::fetch_or_default(&registry).await
            } else {
                RegistryMetadata::default()
            };

            // Resolve backend type
            let backend_type = match registry_config.backend_type() {
                // If the local config specifies a backend type, use it
                Some(backend_type) => Some(backend_type),
                None => {
                    // If the registry metadata indicates a preferred protocol, use it
                    let preferred_protocol = registry_meta.preferred_protocol();
                    // ...except registry metadata cannot force a local backend
                    if preferred_protocol == Some("local") {
                        return Err(Error::InvalidRegistryMetadata(anyhow!(
                            "registry metadata with 'local' protocol not allowed"
                        )));
                    }
                    preferred_protocol
                }
            }
            // Otherwise use the default backend
            .unwrap_or("oci");
            tracing::debug!(?backend_type, "Resolved backend type");

            let source: Box<dyn PackageLoader> = match backend_type {
                "local" => Box::new(LocalBackend::new(registry_config)?),
                "oci" => Box::new(OciBackend::new(
                    &registry,
                    &registry_config,
                    &registry_meta,
                )?),
                "warg" => {
                    Box::new(WargBackend::new(&registry, &registry_config, &registry_meta).await?)
                }
                other => {
                    return Err(Error::InvalidConfig(anyhow!(
                        "unknown backend type {other:?}"
                    )));
                }
            };
            self.sources.insert(registry.clone(), source);
        }
        Ok(self.sources.get_mut(&registry).unwrap().as_mut())
    }
}

/// Package release details.
///
/// Returned by [`Client::get_release`] and passed to [`Client::stream_content`].
#[derive(Clone, Debug)]
pub struct Release {
    pub version: Version,
    pub content_digest: ContentDigest,
}
