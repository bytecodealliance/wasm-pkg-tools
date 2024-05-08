mod toml;

use std::{collections::HashMap, path::PathBuf};

use oci_distribution::client::ClientConfig as OciClientConfig;
use secrecy::SecretString;

use crate::{
    source::{local::LocalConfig, oci::OciConfig, warg::WargConfig},
    Error, PackageRef,
};

/// Configuration for [`super::Client`].
#[derive(Clone, Default)]
pub struct ClientConfig {
    /// The default registry name.
    default_registry: Option<String>,
    /// Per-namespace registry, overriding `default_registry` (if present).
    namespace_registries: HashMap<String, String>,
    /// Per-registry configuration.
    pub(crate) registry_configs: HashMap<String, RegistryConfig>,
}

impl ClientConfig {
    pub fn to_client(&self) -> crate::Client {
        crate::Client::new(self.clone())
    }

    pub fn merge_config(&mut self, other: ClientConfig) -> &mut Self {
        if let Some(default_registry) = other.default_registry {
            self.default_registry(default_registry);
        }
        for (namespace, registry) in other.namespace_registries {
            self.namespace_registry(namespace, registry);
        }
        for (registry, config) in other.registry_configs {
            self.registry_configs.insert(registry, config);
        }
        self
    }

    pub fn default_registry(&mut self, registry: impl Into<String>) -> &mut Self {
        self.default_registry = Some(registry.into());
        self
    }

    pub fn namespace_registry(
        &mut self,
        namespace: impl Into<String>,
        registry: impl Into<String>,
    ) -> &mut Self {
        self.namespace_registries
            .insert(namespace.into(), registry.into());
        self
    }

    pub fn local_registry_config(
        &mut self,
        registry: impl Into<String>,
        root: impl Into<PathBuf>,
    ) -> &mut Self {
        self.registry_configs.insert(
            registry.into(),
            RegistryConfig::Local(LocalConfig { root: root.into() }),
        );
        self
    }

    pub fn oci_registry_config(
        &mut self,
        registry: impl Into<String>,
        client_config: Option<OciClientConfig>,
        credentials: Option<BasicCredentials>,
    ) -> Result<&mut Self, Error> {
        if client_config
            .as_ref()
            .is_some_and(|cfg| cfg.platform_resolver.is_some())
        {
            Error::InvalidConfig(anyhow::anyhow!(
                "oci_distribution::client::ClientConfig::platform_resolver not supported"
            ));
        }
        let cfg = RegistryConfig::Oci(OciConfig {
            client_config,
            credentials,
        });
        self.registry_configs.insert(registry.into(), cfg);
        Ok(self)
    }

    pub fn warg_registry_config(
        &mut self,
        registry: impl Into<String>,
        client_config: Option<warg_client::Config>,
        auth_token: Option<impl Into<SecretString>>,
    ) -> Result<&mut Self, Error> {
        let cfg = RegistryConfig::Warg(WargConfig {
            client_config: client_config.unwrap_or_default(),
            auth_token: auth_token.map(Into::into),
        });
        self.registry_configs.insert(registry.into(), cfg);
        Ok(self)
    }

    pub(crate) fn resolve_package_registry(&self, package: &PackageRef) -> Result<&str, Error> {
        let namespace = package.namespace();
        tracing::debug!("Resolving registry for {namespace:?}");

        if let Some(registry) = self.namespace_registries.get(namespace.as_ref()) {
            tracing::debug!("Found namespace-specific registry {registry:?}");
            return Ok(registry);
        }
        if let Some(registry) = &self.default_registry {
            tracing::debug!("No namespace-specific registry; using default {registry:?}");
            return Ok(registry);
        }
        Err(Error::NoRegistryForNamespace(namespace.to_owned()))
    }
}

/// Configuration for a specific registry.
#[derive(Clone, Debug)]
pub enum RegistryConfig {
    Local(LocalConfig),
    Oci(OciConfig),
    Warg(WargConfig),
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self::Oci(Default::default())
    }
}

#[derive(Clone, Debug)]
pub struct BasicCredentials {
    pub username: String,
    pub password: SecretString,
}
