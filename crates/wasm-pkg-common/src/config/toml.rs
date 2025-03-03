// TODO: caused by inner bytes::Bytes; probably fixed in Rust 1.79
#![allow(clippy::mutable_key_type)]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{label::Label, package::PackageRef, registry::Registry};

use super::RegistryMapping;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TomlConfig {
    default_registry: Option<Registry>,
    #[serde(default)]
    namespace_registries: HashMap<Label, RegistryMapping>,
    #[serde(default)]
    package_registry_overrides: HashMap<PackageRef, RegistryMapping>,
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
            "oci"
        );

        let toml_config = toml::toml! {
            [namespace_registries]
            test = "localhost:1234"

            [package_registry_overrides]

            [registry."localhost:1234".oci]
            auth = { username = "open", password = "sesame" }
            [registry."localhost:1234".other]
            config = "value"
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
            [registry."localhost:1234".oci]
            auth = { username = "open", password = "sesame" }
            [registry."localhost:1234".other]
            config = "value"
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
            [registry."localhost:1234".oci]
            auth = { username = "open", password = "sesame" }
            [registry."localhost:1234".other]
            config = "value"
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

    #[test]
    fn test_custom_namespace_config() {
        let toml_config = toml::toml! {
            [namespace_registries]
            test = { registry = "localhost", metadata = { preferredProtocol = "oci", "oci" = {registry = "ghcr.io", namespacePrefix = "webassembly/" } } }
            foo = "foo:1234"

            [package_registry_overrides]
            "foo:bar" = { registry = "localhost", metadata = { preferredProtocol = "oci", "oci" = {registry = "ghcr.io", namespacePrefix = "webassembly/" } } }

            [registry."localhost".oci]
            auth = { username = "open", password = "sesame" }
        };

        let toml_cfg: TomlConfig = toml_config.try_into().unwrap();
        let cfg = crate::config::Config::from(toml_cfg);

        // First check the the normal string case works
        let ns_config = cfg
            .namespace_registry(&"foo".parse().unwrap())
            .expect("Should have a namespace config");
        let reg = match ns_config {
            RegistryMapping::Registry(r) => r,
            _ => panic!("Should have a registry namespace config"),
        };
        assert_eq!(
            reg,
            &"foo:1234".parse::<Registry>().unwrap(),
            "Should have a registry"
        );

        let ns_config = cfg
            .namespace_registry(&"test".parse().unwrap())
            .expect("Should have a namespace config");
        let custom = match ns_config {
            RegistryMapping::Custom(c) => c,
            _ => panic!("Should have a custom namespace config"),
        };
        assert_eq!(
            custom.registry,
            "localhost".parse().unwrap(),
            "Should have a registry"
        );
        assert_eq!(
            custom.metadata.preferred_protocol(),
            Some("oci"),
            "Should have a preferred protocol"
        );
        // Specific deserializations are tested in the client model
        let map = custom
            .metadata
            .protocol_configs
            .get("oci")
            .expect("Should have a protocol config");
        assert_eq!(
            map.get("registry").expect("registry should exist"),
            "ghcr.io",
            "Should have a registry"
        );
        assert_eq!(
            map.get("namespacePrefix")
                .expect("namespacePrefix should exist"),
            "webassembly/",
            "Should have a namespace prefix"
        );

        // Now test the same thing for a package override
        let ns_config = cfg
            .package_registry_override(&"foo:bar".parse().unwrap())
            .expect("Should have a package override config");
        let custom = match ns_config {
            RegistryMapping::Custom(c) => c,
            _ => panic!("Should have a custom namespace config"),
        };
        assert_eq!(
            custom.registry,
            "localhost".parse().unwrap(),
            "Should have a registry"
        );
        assert_eq!(
            custom.metadata.preferred_protocol(),
            Some("oci"),
            "Should have a preferred protocol"
        );
        // Specific deserializations are tested in the client model
        let map = custom
            .metadata
            .protocol_configs
            .get("oci")
            .expect("Should have a protocol config");
        assert_eq!(
            map.get("registry").expect("registry should exist"),
            "ghcr.io",
            "Should have a registry"
        );
        assert_eq!(
            map.get("namespacePrefix")
                .expect("namespacePrefix should exist"),
            "webassembly/",
            "Should have a namespace prefix"
        );
    }
}
