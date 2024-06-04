use std::{
    collections::{hash_map::Entry, HashMap},
    io::ErrorKind,
    path::Path,
};

use secrecy::SecretString;
use serde::{Deserialize, Serialize};

use crate::{label::Label, package::PackageRef, Error, Registry};

mod toml;

pub const DEFAULT_REGISTRY: &str = "bytecodealliance.org";

/// Wasm Package registry configuration.
///
/// Most consumers are expected to start with [`Config::global_defaults`] to
/// provide a consistent baseline user experience. Where needed, these defaults
/// can be overridden with application-specific config via [`Config::merge`] or
/// other mutation methods.
#[derive(Debug)]
pub struct Config {
    default_registry: Option<Registry>,
    namespace_registries: HashMap<Label, Registry>,
    package_registry_overrides: HashMap<PackageRef, Registry>,
    registry_configs: HashMap<Registry, RegistryConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_registry: Some(DEFAULT_REGISTRY.parse().unwrap()),
            namespace_registries: Default::default(),
            package_registry_overrides: Default::default(),
            registry_configs: Default::default(),
        }
    }
}

impl Config {
    /// Returns an empty config.
    ///
    /// Note that this may differ from the `Default` implementation, which
    /// includes hard-coded global defaults.
    pub fn new() -> Self {
        Self {
            default_registry: Default::default(),
            namespace_registries: Default::default(),
            package_registry_overrides: Default::default(),
            registry_configs: Default::default(),
        }
    }

    /// Loads config from several default sources.
    ///
    /// The following sources are loaded in this order, with later sources
    /// merged into (overriding) earlier sources.
    /// - Hard-coded defaults
    /// - User-global config file (e.g. `~/.config/wasm-pkg/config.toml`)
    ///
    /// Note: This list is expected to expand in the future to include
    /// "workspace" config files like `./.wasm-pkg/config.toml`.
    pub fn global_defaults() -> Result<Self, Error> {
        let mut config = Self::default();
        if let Some(global_config) = Self::read_global_config()? {
            config.merge(global_config);
        }
        Ok(config)
    }

    /// Reads config from
    pub fn read_global_config() -> Result<Option<Self>, Error> {
        let Some(config_dir) = dirs::config_dir() else {
            return Ok(None);
        };
        let path = config_dir.join("wasm-pkg").join("config.toml");
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(Error::ConfigFileIoError(err)),
        };
        Ok(Some(Self::from_toml(&contents)?))
    }

    /// Reads config from a TOML file at the given path.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, Error> {
        let contents = std::fs::read_to_string(path).map_err(Error::ConfigFileIoError)?;
        Self::from_toml(&contents)
    }

    /// Parses config from the given TOML contents.
    pub fn from_toml(contents: &str) -> Result<Self, Error> {
        let toml_cfg: toml::TomlConfig =
            ::toml::from_str(contents).map_err(Error::invalid_config)?;
        Ok(toml_cfg.into())
    }

    /// Merges the given other config into this one.
    pub fn merge(&mut self, other: Self) {
        let Self {
            default_registry,
            namespace_registries,
            package_registry_overrides: package_registries,
            registry_configs,
        } = other;
        if default_registry.is_some() {
            self.default_registry = default_registry;
        }
        self.namespace_registries.extend(namespace_registries);
        self.package_registry_overrides.extend(package_registries);
        for (registry, config) in registry_configs {
            match self.registry_configs.entry(registry) {
                Entry::Occupied(mut occupied) => occupied.get_mut().merge(config),
                Entry::Vacant(vacant) => {
                    vacant.insert(config);
                }
            }
        }
    }

    /// Resolves a [`Registry`] for the given [`PackageRef`].
    ///
    /// Resolution returns the first of these that matches:
    /// - A package registry exactly matching the package
    /// - A namespace registry matching the package's namespace
    /// - The default registry
    pub fn resolve_registry(&self, package: &PackageRef) -> Option<&Registry> {
        if let Some(reg) = self.package_registry_overrides.get(package) {
            Some(reg)
        } else if let Some(reg) = self.namespace_registries.get(package.namespace()) {
            Some(reg)
        } else if let Some(reg) = self.default_registry.as_ref() {
            Some(reg)
        } else {
            None
        }
    }

    /// Returns the default registry.
    pub fn default_registry(&self) -> Option<&Registry> {
        self.default_registry.as_ref()
    }

    /// Sets the default registry.
    pub fn set_default_registry(&mut self, registry: Option<Registry>) {
        self.default_registry = registry;
    }

    /// Returns a registry for the given namespace.
    ///
    /// Does not fall back to the default registry; see [`Self::resolve`].
    pub fn namespace_registry(&self, namespace: &Label) -> Option<&Registry> {
        self.namespace_registries.get(namespace)
    }

    /// Sets a registry for the given namespace.
    pub fn set_namespace_registry(&mut self, namespace: Label, registry: Registry) {
        self.namespace_registries.insert(namespace, registry);
    }

    /// Returns a registry override configured for the given package.
    ///
    /// Does not fall back to namespace or default registries; see [`Self::resolve`].
    pub fn package_registry_override(&self, package: &PackageRef) -> Option<&Registry> {
        self.package_registry_overrides.get(package)
    }

    /// Sets a registry override for the given package.
    pub fn set_package_registry_override(&mut self, package: PackageRef, registry: Registry) {
        self.package_registry_overrides.insert(package, registry);
    }

    /// Returns [`RegistryConfig`] for the given registry.
    pub fn registry_config(&self, registry: &Registry) -> Option<&RegistryConfig> {
        self.registry_configs.get(registry)
    }

    /// Returns a mutable [`RegistryConfig`] for the given registry, inserting
    /// an empty one if needed.
    pub fn get_or_insert_registry_config_mut(
        &mut self,
        registry: &Registry,
    ) -> &mut RegistryConfig {
        if !self.registry_configs.contains_key(registry) {
            self.registry_configs
                .insert(registry.clone(), Default::default());
        }
        self.registry_configs.get_mut(registry).unwrap()
    }
}

#[derive(Default)]
pub struct RegistryConfig {
    backend_type: Option<String>,
    backend_configs: HashMap<String, ::toml::Table>,
}

impl RegistryConfig {
    /// Merges the given other config into this one.
    pub fn merge(&mut self, other: Self) {
        let Self {
            backend_type,
            backend_configs,
        } = other;
        if backend_type.is_some() {
            self.backend_type = backend_type;
        }
        for (ty, config) in backend_configs {
            match self.backend_configs.entry(ty) {
                Entry::Occupied(mut occupied) => occupied.get_mut().extend(config),
                Entry::Vacant(vacant) => {
                    vacant.insert(config);
                }
            }
        }
    }

    /// Returns the backend type override.
    pub fn backend_type(&self) -> Option<&str> {
        self.backend_type.as_deref()
    }

    /// Sets the backend type override.
    pub fn set_backend_type(&mut self, backend_type: Option<String>) {
        self.backend_type = backend_type;
    }

    /// Returns an iterator of configured backend types.
    pub fn configured_backend_types(&self) -> impl Iterator<Item = &str> {
        self.backend_configs.keys().map(|ty| ty.as_str())
    }

    /// Attempts to deserialize backend config with the given type.
    ///
    /// Returns `Ok(None)` if no configuration was provided.
    /// Returns `Err` if configuration was provided but deserialization failed.
    pub fn backend_config<'a, T: Deserialize<'a>>(
        &'a self,
        backend_type: &str,
    ) -> Result<Option<T>, Error> {
        let Some(table) = self.backend_configs.get(backend_type) else {
            return Ok(None);
        };
        let config = table.clone().try_into().map_err(Error::invalid_config)?;
        Ok(Some(config))
    }

    /// Set the backend config of the given type by serializing the given config.
    pub fn set_backend_config<T: Serialize>(
        &mut self,
        backend_type: String,
        backend_config: T,
    ) -> Result<(), Error> {
        let table = ::toml::Table::try_from(backend_config).map_err(Error::invalid_config)?;
        self.backend_configs.insert(backend_type, table);
        Ok(())
    }
}

impl std::fmt::Debug for RegistryConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryConfig")
            .field("backend_type", &self.backend_type)
            .field(
                "backend_configs",
                &DebugBackendConfigs(&self.backend_configs),
            )
            .finish()
    }
}

// Redact backend configs, which may contain sensitive values.
struct DebugBackendConfigs<'a>(&'a HashMap<String, ::toml::Table>);

impl<'a> std::fmt::Debug for DebugBackendConfigs<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.0.keys().map(|ty| (ty, &"<HIDDEN>")))
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct BasicCredentials {
    pub username: String,
    pub password: SecretString,
}
