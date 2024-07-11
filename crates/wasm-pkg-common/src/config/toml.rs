// TODO: caused by inner bytes::Bytes; probably fixed in Rust 1.79
#![allow(clippy::mutable_key_type)]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{label::Label, package::PackageRef, registry::Registry};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TomlConfig {
    default_registry: Option<Registry>,
    #[serde(default)]
    namespace_registries: HashMap<Label, Registry>,
    #[serde(default)]
    package_registry_overrides: HashMap<PackageRef, Registry>,
    #[serde(default)]
    registry: HashMap<Registry, TomlRegistryConfig>,
}

impl From<TomlConfig> for super::Config {
    fn from(value: TomlConfig) -> Self {
        let TomlConfig {
            default_registry,
            namespace_registries,
            package_registry_overrides,
            registry,
        } = value;

        let registry_configs = registry
            .into_iter()
            .map(|(reg, config)| (reg, config.into()))
            .collect();

        Self {
            default_registry,
            namespace_registries,
            package_registry_overrides,
            fallback_namespace_registries: Default::default(),
            registry_configs,
        }
    }
}

impl From<super::Config> for TomlConfig {
    fn from(value: super::Config) -> Self {
        let registry = value
            .registry_configs
            .into_iter()
            .map(|(reg, config)| (reg, config.into()))
            .collect();
        Self {
            default_registry: value.default_registry,
            namespace_registries: value.namespace_registries,
            package_registry_overrides: value.package_registry_overrides,
            registry,
        }
    }
}

#[derive(Deserialize, Serialize)]
struct TomlRegistryConfig {
    #[serde(alias = "type")]
    default: Option<String>,
    #[serde(flatten)]
    backend_configs: HashMap<String, toml::Table>,
}

impl From<TomlRegistryConfig> for super::RegistryConfig {
    fn from(value: TomlRegistryConfig) -> Self {
        let TomlRegistryConfig {
            default,
            backend_configs,
        } = value;
        Self {
            default_backend: default,
            backend_configs,
        }
    }
}

impl From<super::RegistryConfig> for TomlRegistryConfig {
    fn from(value: super::RegistryConfig) -> Self {
        let super::RegistryConfig {
            default_backend: backend_default,
            backend_configs,
        } = value;
        Self {
            default: backend_default,
            backend_configs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_test() {
        let toml_config = toml::toml! {
            default_registry = "example.com"

            [namespace_registries]
            wasi = "wasi.dev"

            [package_registry_overrides]
            "example:foo" = "example.com"

            [registry."wasi.dev".oci]
            auth = { username = "open", password = "sesame" }

            [registry."example.com"]
            type = "test"
            test = { token = "top_secret" }
        };
        let wasi_dev: Registry = "wasi.dev".parse().unwrap();
        let example_com: Registry = "example.com".parse().unwrap();

        let toml_cfg: TomlConfig = toml_config.try_into().unwrap();
        let cfg = crate::config::Config::from(toml_cfg);

        assert_eq!(cfg.default_registry(), Some(&example_com));
        assert_eq!(
            cfg.resolve_registry(&"wasi:http".parse().unwrap()),
            Some(&wasi_dev)
        );
        assert_eq!(
            cfg.resolve_registry(&"example:foo".parse().unwrap()),
            Some(&example_com)
        );

        #[derive(Deserialize)]
        struct TestConfig {
            token: String,
        }
        let test_cfg: TestConfig = cfg
            .registry_config(&example_com)
            .unwrap()
            .backend_config("test")
            .unwrap()
            .unwrap();
        assert_eq!(test_cfg.token, "top_secret");
    }

    #[test]
    fn type_parses_correctly() {
        let toml_config = toml::toml! {
            [namespace_registries]
            test = "localhost:1234"

            [package_registry_overrides]

            [registry."localhost:1234".warg]
            config_file = "/a/path"
        };

        let toml_cfg: TomlConfig = toml_config.try_into().unwrap();
        let cfg = crate::config::Config::from(toml_cfg);
        let reg_conf = cfg
            .registry_config(&"localhost:1234".parse().unwrap())
            .expect("Should have config for registry");
        assert_eq!(
            reg_conf
                .default_backend()
                .expect("Should have a default set"),
            "warg"
        );

        let toml_config = toml::toml! {
            [namespace_registries]
            test = "localhost:1234"

            [package_registry_overrides]

            [registry."localhost:1234".warg]
            config_file = "/a/path"
            [registry."localhost:1234".oci]
            auth = { username = "open", password = "sesame" }
        };

        let toml_cfg: TomlConfig = toml_config.try_into().unwrap();
        let cfg = crate::config::Config::from(toml_cfg);
        let reg_conf = cfg
            .registry_config(&"localhost:1234".parse().unwrap())
            .expect("Should have config for registry");
        assert!(
            reg_conf.default_backend().is_none(),
            "Should not have a type set when two configs exist"
        );

        let toml_config = toml::toml! {
            [namespace_registries]
            test = "localhost:1234"

            [package_registry_overrides]

            [registry."localhost:1234"]
            type = "foobar"
            [registry."localhost:1234".warg]
            config_file = "/a/path"
            [registry."localhost:1234".oci]
            auth = { username = "open", password = "sesame" }
        };

        let toml_cfg: TomlConfig = toml_config.try_into().unwrap();
        let cfg = crate::config::Config::from(toml_cfg);
        let reg_conf = cfg
            .registry_config(&"localhost:1234".parse().unwrap())
            .expect("Should have config for registry");
        assert_eq!(
            reg_conf
                .default_backend()
                .expect("Should have a default set using the type alias"),
            "foobar"
        );

        let toml_config = toml::toml! {
            [namespace_registries]
            test = "localhost:1234"

            [registry."localhost:1234"]
            default = "foobar"
            [registry."localhost:1234".warg]
            config_file = "/a/path"
            [registry."localhost:1234".oci]
            auth = { username = "open", password = "sesame" }
        };

        let toml_cfg: TomlConfig = toml_config.try_into().unwrap();
        let cfg = crate::config::Config::from(toml_cfg);
        let reg_conf = cfg
            .registry_config(&"localhost:1234".parse().unwrap())
            .expect("Should have config for registry");
        assert_eq!(
            reg_conf
                .default_backend()
                .expect("Should have a default set"),
            "foobar"
        );
    }
}
