mod config;
mod label;
mod meta;
mod package;
mod release;
mod source;

use std::collections::HashMap;

use bytes::Bytes;
use futures_util::stream::BoxStream;
use oci_distribution::errors::OciDistributionError;
pub use semver::Version;
use source::{
    local::LocalSource,
    oci::{OciConfig, OciSource},
    warg::{WargConfig, WargSource},
    PackageSource,
};

/// Re-exported to ease configuration.
pub use oci_distribution::client as oci_client;

pub use crate::{
    config::ClientConfig,
    package::PackageRef,
    release::{ContentDigest, Release},
};
use crate::{
    config::RegistryConfig,
    label::{InvalidLabel, Label},
    meta::RegistryMeta,
};

/// A read-only registry client.
pub struct Client {
    config: ClientConfig,
    sources: HashMap<String, Box<dyn PackageSource>>,
}

impl Client {
    /// Returns a new client with the given [`ClientConfig`].
    pub fn new(config: ClientConfig) -> Self {
        Self {
            config,
            sources: Default::default(),
        }
    }

    /// Returns a new client configured from the default config file path.
    /// Returns Ok(None) if the default config file does not exist.
    pub fn from_default_config_file() -> Result<Option<Self>, Error> {
        Ok(ClientConfig::from_default_file()?.map(Self::new))
    }

    /// Returns a list of all package [`Version`]s available for the given package.
    pub async fn list_all_versions(&mut self, package: &PackageRef) -> Result<Vec<Version>, Error> {
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
    ) -> Result<&mut dyn PackageSource, Error> {
        let registry = self.config.resolve_package_registry(package)?.to_owned();
        if !self.sources.contains_key(&registry) {
            let registry_config = self.config.registry_configs.get(&registry).cloned();

            tracing::debug!("Resolved registry config: {registry_config:?}");

            let registry_meta = RegistryMeta::fetch_or_default(&registry).await;

            let registry_config = registry_config.unwrap_or_else(|| {
                if registry_meta.warg_url.is_some() {
                    RegistryConfig::Warg(Default::default())
                } else {
                    RegistryConfig::Oci(Default::default())
                }
            });

            let source: Box<dyn PackageSource> = match registry_config {
                config::RegistryConfig::Local(config) => Box::new(LocalSource::new(config)),
                config::RegistryConfig::Oci(config) => {
                    Box::new(self.build_oci_client(&registry, config).await?)
                }
                config::RegistryConfig::Warg(config) => {
                    Box::new(self.build_warg_client(&registry, config).await?)
                }
            };
            self.sources.insert(registry.clone(), source);
        }
        Ok(self.sources.get_mut(&registry).unwrap().as_mut())
    }

    async fn build_oci_client(
        &mut self,
        registry: &str,
        config: OciConfig,
    ) -> Result<OciSource, Error> {
        tracing::debug!("Building new OCI client for {registry:?}");
        // Check registry metadata for OCI registry override
        let registry_meta = RegistryMeta::fetch_or_default(registry).await;
        OciSource::new(registry.to_string(), config, registry_meta)
    }

    async fn build_warg_client(
        &mut self,
        registry: &str,
        config: WargConfig,
    ) -> Result<WargSource, Error> {
        tracing::debug!("Building new Warg client for {registry:?}");
        // Check registry metadata for OCI registry override
        let registry_meta = RegistryMeta::fetch_or_default(registry).await;
        WargSource::new(registry.to_string(), config, registry_meta)
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("failed to get registry credentials: {0:#}")]
    CredentialError(anyhow::Error),
    #[error("invalid config: {0:#}")]
    InvalidConfig(anyhow::Error),
    #[error("invalid content: {0}")]
    InvalidContent(String),
    #[error("invalid content digest: {0}")]
    InvalidContentDigest(String),
    #[error("invalid label: {0}")]
    InvalidLabel(#[from] InvalidLabel),
    #[error("invalid package ref: {0}")]
    InvalidPackageRef(String),
    #[error("invalid package manifest: {0}")]
    InvalidPackageManifest(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("OCI error: {0}")]
    OciError(#[from] OciDistributionError),
    #[error("no registry configured for namespace {0:?}")]
    NoRegistryForNamespace(Label),
    #[error("registry metadata error: {0:#}")]
    RegistryMeta(#[source] anyhow::Error),
    #[error("invalid version: {0}")]
    VersionError(#[from] semver::Error),
    #[error("Warg error: {0}")]
    WargError(#[from] warg_client::ClientError),
}
