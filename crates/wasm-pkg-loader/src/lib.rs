//! A crate for loading wasm components and packages from a registry
pub mod config;
mod release;
pub(crate) mod source;

use std::collections::HashMap;

use bytes::Bytes;
use futures_util::stream::BoxStream;
pub use semver::Version;
use source::oci::OciConfig;
use wasm_pkg_common::{
    config::Config,
    package::PackageRef,
    registry::{
        OciProtocolConfig, RegistryMetadata, WargProtocolConfig, OCI_PROTOCOL, WARG_PROTOCOL,
    },
    Error,
};

/// Re-exported to ease configuration.
pub use oci_distribution::client as oci_client;

use crate::config::{
    oci::OciRegistryConfig,
    warg::{WargRawConfig, WargRegistryConfig},
};
pub use crate::release::{ContentDigest, Release};
use crate::source::{
    oci::OciSource,
    warg::{WargConfig, WargSource},
    PackageSource, VersionInfo,
};

/// A read-only registry client.
pub struct Client {
    config: Config,
    sources: HashMap<String, Box<dyn PackageSource>>,
}

impl Client {
    /// Returns a new client with the given [`Config`].
    pub fn new(config: Config) -> Self {
        Self {
            config,
            sources: Default::default(),
        }
    }

    /// Returns a new client configured from the default config file path.
    /// Returns Ok(None) if the default config file does not exist.
    pub fn from_default_config_file() -> Result<Option<Self>, Error> {
        Ok(Config::read_global_config()?.map(Self::new))
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
    ) -> Result<&mut dyn PackageSource, Error> {
        let registry = self
            .config
            .resolve_registry(package)
            .ok_or_else(|| {
                Error::InvalidPackageRef(format!("Couldn't resolve registry for {package}"))
            })?
            .to_owned();
        if !self.sources.contains_key(registry.as_ref()) {
            let registry_config = self.config.registry_config(&registry);

            tracing::debug!(?registry_config, "Resolved registry config");

            let maybe_metadata = RegistryMetadata::fetch(registry.as_ref()).await?;

            let source: Box<dyn PackageSource> = match registry_config {
                Some(conf) if conf.backend_type().unwrap_or_default() == "oci" => {
                    let conf: OciRegistryConfig = conf.backend_config("oci")?.ok_or_else(|| {
                        Error::InvalidConfig(anyhow::anyhow!(
                            "No OCI config found for registry {registry}"
                        ))
                    })?;
                    let meta = maybe_metadata
                        .and_then(|metadata| {
                            metadata
                                .protocol_config::<OciProtocolConfig>(OCI_PROTOCOL)
                                .ok()
                        })
                        .flatten();
                    Box::new(self.build_oci_client(registry.as_ref(), meta, conf.into())?)
                }
                Some(conf) if conf.backend_type().unwrap_or_default() == "warg" => {
                    let meta = maybe_metadata
                        .and_then(|metadata| {
                            metadata
                                .protocol_config::<WargProtocolConfig>(WARG_PROTOCOL)
                                .ok()
                        })
                        .flatten();
                    let conf: WargRawConfig = conf.backend_config("warg")?.ok_or_else(|| {
                        Error::InvalidConfig(anyhow::anyhow!(
                            "No warg config found for registry {registry}"
                        ))
                    })?;
                    let conf: WargRegistryConfig = conf.try_into()?;
                    Box::new(
                        self.build_warg_client(
                            registry.as_ref(),
                            meta,
                            WargConfig {
                                client_config: Some(conf.client_config),
                                auth_token: conf.auth_token,
                            },
                        )
                        .await?,
                    )
                }
                Some(_) | None => {
                    if let Some(meta) = maybe_metadata
                        .as_ref()
                        .and_then(|meta| {
                            meta.protocol_config::<WargProtocolConfig>(WARG_PROTOCOL)
                                .ok()
                        })
                        .flatten()
                    {
                        Box::new(
                            self.build_warg_client(
                                registry.as_ref(),
                                Some(meta),
                                Default::default(),
                            )
                            .await?,
                        )
                    } else {
                        // Default to OCI client
                        let meta = maybe_metadata
                            .and_then(|meta| {
                                meta.protocol_config::<OciProtocolConfig>(OCI_PROTOCOL).ok()
                            })
                            .flatten();
                        Box::new(self.build_oci_client(
                            registry.as_ref(),
                            meta,
                            Default::default(),
                        )?)
                    }
                }
            };
            self.sources.insert(registry.to_string(), source);
        }
        Ok(self.sources.get_mut(registry.as_ref()).unwrap().as_mut())
    }

    fn build_oci_client(
        &mut self,
        registry: &str,
        registry_meta: Option<OciProtocolConfig>,
        config: OciConfig,
    ) -> Result<OciSource, Error> {
        tracing::debug!(?registry, "Building new OCI client");
        Ok(OciSource::new(registry.to_string(), config, registry_meta))
    }

    async fn build_warg_client(
        &mut self,
        registry: &str,
        registry_meta: Option<WargProtocolConfig>,
        config: WargConfig,
    ) -> Result<WargSource, Error> {
        tracing::debug!(?registry, "Building new Warg client");
        WargSource::new(registry.to_string(), config, registry_meta).await
    }
}
