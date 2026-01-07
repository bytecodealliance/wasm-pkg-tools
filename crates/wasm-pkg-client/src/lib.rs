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
mod loader;
pub mod local;
pub mod metadata;
pub mod oci;
mod publisher;
mod release;
pub mod warg;

use std::path::Path;
use std::sync::Arc;
use std::{collections::HashMap, pin::Pin};

use anyhow::anyhow;
use bytes::Bytes;
use futures_util::Stream;
use publisher::PackagePublisher;
use tokio::io::AsyncSeekExt;
use tokio::sync::RwLock;
use tokio_util::io::SyncIoBridge;
pub use wasm_pkg_common::{
    config::{Config, CustomConfig, RegistryMapping},
    digest::ContentDigest,
    metadata::RegistryMetadata,
    package::{PackageRef, Version},
    registry::Registry,
    Error,
};
use wit_component::DecodedWasm;

use crate::metadata::RegistryMetadataExt;
use crate::{loader::PackageLoader, local::LocalBackend, oci::OciBackend, warg::WargBackend};

pub use release::{Release, VersionInfo};

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
        let (data, package, version) = if let Some((p, v)) = additional_options.package {
            (data, p, v)
        } else {
            let data = SyncIoBridge::new(data);
            let (mut data, p, v) = tokio::task::spawn_blocking(|| resolve_package(data))
                .await
                .map_err(|e| {
                    crate::Error::IoError(std::io::Error::other(format!(
                        "Error when performing blocking IO: {e:?}"
                    )))
                })??;
            // We must rewind the reader because we read to the end to parse the component.
            data.rewind().await?;
            (data, p, v)
        };
        let source = self
            .resolve_source(&package, additional_options.registry)
            .await?;
        source
            .publish(&package, &version, data)
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

/// Resolves the package name and version from the given source. This takes a wrapped publishing
/// source to it can do a blocking read with wit_component. It returns back the underlying
/// PublishingSource but should be rewound to the beginning of the source
fn resolve_package(
    mut data: SyncIoBridge<PublishingSource>,
) -> Result<(PublishingSource, PackageRef, Version), Error> {
    let (resolve, package_id) =
        match wit_component::decode_reader(&mut data).map_err(crate::Error::InvalidComponent)? {
            DecodedWasm::Component(resolve, world_id) => {
                let package_id = resolve
                    .worlds
                    .iter()
                    .find_map(|(id, w)| if id == world_id { w.package } else { None })
                    .ok_or_else(|| {
                        crate::Error::InvalidComponent(anyhow::anyhow!(
                            "component world or package not found"
                        ))
                    })?;
                (resolve, package_id)
            }
            DecodedWasm::WitPackage(resolve, package_id) => (resolve, package_id),
        };
    let (package, version) = resolve
        .package_names
        .into_iter()
        .find_map(|(pkg, id)| {
            // SAFETY: We just parsed this from wit and should be able to unwrap. If it
            // isn't a valid identifier, something else is majorly wrong
            (id == package_id).then(|| {
                (
                    PackageRef::new(
                        pkg.namespace.try_into().unwrap(),
                        pkg.name.try_into().unwrap(),
                    ),
                    pkg.version,
                )
            })
        })
        .ok_or_else(|| {
            crate::Error::InvalidComponent(anyhow::anyhow!("component package not found"))
        })?;

    let version = version.ok_or_else(|| {
        crate::Error::InvalidComponent(anyhow::anyhow!("component package version not found"))
    })?;
    Ok((data.into_inner(), package, version))
}
