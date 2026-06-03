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
//! let mut client = wasm_pkg_client::Client::with_global_defaults().await?;
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

pub mod caching;
mod decoded_component;
mod loader;
pub mod local;
pub mod metadata;
pub mod oci;
mod publisher;
mod release;
pub mod warg;

use crate::{
    loader::{PackageLoader, VersionSort},
    local::LocalBackend,
    metadata::RegistryMetadataExt,
    oci::OciBackend,
    warg::WargBackend,
};
use anyhow::anyhow;
use bytes::Bytes;
use decoded_component::DecodedComponent;
use futures_concurrency::prelude::*;
use futures_util::Stream;
use publisher::PackagePublisher;
pub use release::{Release, VersionInfo};
use std::{cmp::Ordering, collections::HashMap, path::Path, pin::Pin, sync::Arc};
use tokio::sync::RwLock;
pub use wasm_pkg_common::{
    config::{Config, CustomConfig, RegistryMapping},
    digest::ContentDigest,
    metadata::RegistryMetadata,
    package::{PackageRef, Version, VersionReq},
    registry::Registry,
    Error,
};

/// An alias for a stream of content bytes
pub type ContentStream = Pin<Box<dyn Stream<Item = Result<Bytes, Error>> + Send + 'static>>;

/// An alias for a PublishingSource (generally a file)
pub type PublishingSource = Pin<Box<dyn ReaderSeeker + Send + Sync + 'static>>;

/// A supertrait combining tokio's AsyncRead and AsyncSeek.
pub trait ReaderSeeker: tokio::io::AsyncRead + tokio::io::AsyncSeek {}
impl<T> ReaderSeeker for T where T: tokio::io::AsyncRead + tokio::io::AsyncSeek {}

trait LoaderPublisher: PackageLoader + PackagePublisher {}

impl<T> LoaderPublisher for T where T: PackageLoader + PackagePublisher {}

type RegistrySources = HashMap<Registry, Arc<InnerClient>>;
type InnerClient = Box<dyn LoaderPublisher + Sync>;

/// Additional options for publishing a package.
#[derive(Clone, Debug, Default)]
pub struct PublishOpts {
    /// Override the package name and version to publish with.
    pub package: Option<(PackageRef, Version)>,
    /// Override the registry to publish to.
    pub registry: Option<Registry>,
    /// If true, resolve the package, version, and registry but do not call the
    /// backend to publish.
    pub dry_run: bool,
    /// Disable semver compatibility verification.
    pub skip_semver_check: bool,
}

/// A read-only registry client.
#[derive(Clone)]
pub struct Client {
    config: Arc<Config>,
    sources: Arc<RwLock<RegistrySources>>,
}

impl Client {
    /// Returns a new client with the given [`Config`].
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(config),
            sources: Default::default(),
        }
    }

    /// Returns a reference to the configuration this client was initialized with.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Returns a new client configured from default global config.
    pub async fn with_global_defaults() -> Result<Self, Error> {
        let config = Config::global_defaults().await?;
        Ok(Self::new(config))
    }

    /// Returns a list of all package [`Version`]s available for the given package.
    pub async fn list_all_versions(&self, package: &PackageRef) -> Result<Vec<VersionInfo>, Error> {
        let source = self.resolve_source(package, None).await?;
        source.list_all_versions(package).await
    }

    /// Returns a [`Release`] for the given package version.
    pub async fn get_release(
        &self,
        package: &PackageRef,
        version: &Version,
    ) -> Result<Release, Error> {
        let source = self.resolve_source(package, None).await?;
        source.get_release(package, version).await
    }

    /// Returns a [`ContentStream`] of content chunks. Contents are validated
    /// against the given [`Release::content_digest`].
    pub async fn stream_content<'a>(
        &'a self,
        package: &'a PackageRef,
        release: &'a Release,
    ) -> Result<ContentStream, Error> {
        let source = self.resolve_source(package, None).await?;
        source.stream_content(package, release).await
    }

    /// Publishes the given file as a package release. The package name and version will be read
    /// from the component if not given as part of `additional_options`. Returns the package name
    /// and version of the published release.
    pub async fn publish_release_file(
        &self,
        file: impl AsRef<Path>,
        additional_options: PublishOpts,
    ) -> Result<(PackageRef, Version), Error> {
        let data = tokio::fs::OpenOptions::new().read(true).open(file).await?;

        self.publish_release_data(Box::pin(data), additional_options)
            .await
    }

    /// Publishes the given reader as a package release. TThe package name and version will be read
    /// from the component if not given as part of `additional_options`. Returns the package name
    /// and version of the published release.
    pub async fn publish_release_data(
        &self,
        data: PublishingSource,
        additional_options: PublishOpts,
    ) -> Result<(PackageRef, Version), Error> {
        // handle opts
        let registry = additional_options.registry;
        let semver_check: bool = additional_options.skip_semver_check;
        let pkg_authority = additional_options.package;

        // construct verificable publishing source
        let (data, candidate) =
            DecodedComponent::from_publishing_source_with_package(data, pkg_authority).await?;

        let (package, version) = (
            candidate.package().to_owned(),
            candidate.version().to_owned(),
        );

        // instantiate LoaderPublisher
        let source = self.resolve_source(&package, registry).await?;

        // execute pre-flight
        if !semver_check {
            // fetch nearest neighbors of interest
            let mut neighbors: [Option<VersionInfo>; 2] = [None, None];
            for version_info in
                fetch_semver_series(source.as_ref().as_ref(), &package, &version).await?
            {
                match version.cmp(&version_info.version) {
                    Ordering::Equal => return Err(Error::VersionAlreadyExists(version.to_owned())),
                    Ordering::Greater => {
                        neighbors[0] = Some(version_info);
                        break;
                    }
                    Ordering::Less => {
                        neighbors[1] = Some(version_info);
                    }
                }
            }

            // queue up load/decode futures
            let prepare_neighbor_ops: Vec<_> = neighbors
                .into_iter()
                .flatten()
                .map(|v| fetch_and_resolve_package(&**source, &package, v.version))
                .collect();

            // execute load/decode ops, collect results.
            let mut semver_series: Vec<decoded_component::DecodedComponent> = prepare_neighbor_ops
                .join()
                .await
                .into_iter()
                .collect::<Result<_, _>>()?;

            // verify candidate is in compliance with its semver neighbors
            if !semver_series.is_empty() {
                semver_series.push(candidate);

                semver_series.sort_by(|a, b| a.version().cmp(b.version()));
                for window in semver_series.windows(2) {
                    let [prev, next] = window else { unreachable!() };
                    prev.semver_check(next)?;
                }
            }
        }

        source
            .publish(&package, &version, data, additional_options.dry_run)
            .await
            .map(|_| (package, version))
    }

    async fn resolve_source(
        &self,
        package: &PackageRef,
        registry_override: Option<Registry>,
    ) -> Result<Arc<InnerClient>, Error> {
        let is_override = registry_override.is_some();
        let registry = if let Some(registry) = registry_override {
            registry
        } else {
            self.config
                .resolve_registry(package)
                .ok_or_else(|| Error::NoRegistryForNamespace(package.namespace().clone()))?
                .to_owned()
        };
        let has_key = {
            let sources = self.sources.read().await;
            sources.contains_key(&registry)
        };
        if !has_key {
            let registry_config = self
                .config
                .registry_config(&registry)
                .cloned()
                .unwrap_or_default();

            // Skip fetching metadata for "local" source
            let should_fetch_meta = registry_config.default_backend() != Some("local");
            let maybe_metadata = self
                .config
                .package_registry_override(package)
                .and_then(|mapping| match mapping {
                    RegistryMapping::Custom(custom) => Some(custom.metadata.clone()),
                    _ => None,
                })
                .or_else(|| {
                    self.config
                        .namespace_registry(package.namespace())
                        .and_then(|meta| {
                            // If the overridden registry matches the registry we are trying to resolve, we
                            // should use the metadata, otherwise we'll need to fetch the metadata from the
                            // registry
                            match (meta, is_override) {
                                (RegistryMapping::Custom(custom), true)
                                    if custom.registry == registry =>
                                {
                                    Some(custom.metadata.clone())
                                }
                                (RegistryMapping::Custom(custom), false) => {
                                    Some(custom.metadata.clone())
                                }
                                _ => None,
                            }
                        })
                });

            let registry_meta = if let Some(meta) = maybe_metadata {
                meta
            } else if should_fetch_meta {
                RegistryMetadata::fetch_or_default(&registry).await
            } else {
                RegistryMetadata::default()
            };

            // Resolve backend type
            let backend_type = match registry_config.default_backend() {
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

            let source: InnerClient = match backend_type {
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
            self.sources
                .write()
                .await
                .insert(registry.clone(), Arc::new(source));
        }
        Ok(self.sources.read().await.get(&registry).unwrap().clone())
    }
}

// Fetch every prior release in the same semver compatibility series as
// `version`, sorted in descending order.
//
// The "series" here is the cargo-style `^` compat range, not just
// `~major.minor.*`. We can be that permissive because
// `wit_component::semver_check` is *structural*: it strips every package
// version to `None` before comparing, so it enforces additive-only changes
// regardless of how the version numbers move. The only thing the version
// numbers are used for here is *which neighbors to check against* — and the
// semver contract already says everything inside a compat range must be
// additive, so that's the right gate.
//
//   X.y.z (X >= 1) -> X.*       (minors are additive within a major)
//   0.Y.z (Y >= 1) -> 0.Y.*     (in 0.x, minor bumps are breaking)
//   0.0.Z          -> 0.0.Z     (every patch is its own series)
async fn fetch_semver_series(
    source: &(dyn LoaderPublisher + Sync),
    package: &PackageRef,
    version: &Version,
) -> Result<Vec<VersionInfo>, Error> {
    let mask = if version.major > 0 {
        format!("{}.*", version.major)
    } else if version.minor > 0 {
        format!("0.{}.*", version.minor)
    } else {
        version.to_string()
    };
    let req = VersionReq::parse(&mask)
        .map_err(|e| Error::InvalidConfig(anyhow!("invalid version mask: {e}")))?;

    source
        .list_matching_versions(package, req, VersionSort::Descending)
        .await
}

// fetch a package from
async fn fetch_and_resolve_package(
    source: &(dyn LoaderPublisher + Sync),
    package: &PackageRef,
    version: Version,
) -> Result<decoded_component::DecodedComponent, Error> {
    let stream = source
        .stream_content(package, &source.get_release(package, &version).await?)
        .await
        .map_err(std::io::Error::other)?;

    DecodedComponent::from_content_stream(stream, package.clone(), version).await
}
