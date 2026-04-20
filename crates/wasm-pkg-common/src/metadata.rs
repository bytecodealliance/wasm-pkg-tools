use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap},
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::Error;

/// Well-Known URI (RFC 8615) path for registry metadata.
pub const REGISTRY_METADATA_PATH: &str = "/.well-known/wasm-pkg/registry.json";

type JsonObject = serde_json::Map<String, serde_json::Value>;

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryMetadata {
    /// The registry's preferred protocol.
    pub preferred_protocol: Option<String>,

    /// Protocol-specific configuration.
    #[serde(flatten)]
    pub protocol_configs: HashMap<String, JsonObject>,

    // Backward-compatibility aliases:
    /// OCI Registry
    #[serde(skip_serializing)]
    oci_registry: Option<String>,

    /// OCI Namespace Prefix
    #[serde(skip_serializing)]
    oci_namespace_prefix: Option<String>,
}

const OCI_PROTOCOL: &str = "oci";

impl RegistryMetadata {
    /// Returns the registry's preferred protocol.
    ///
    /// The preferred protocol is:
    /// - the `preferredProtocol` metadata field, if given
    /// - the protocol configuration key, if only one configuration is given
    /// - the protocol backward-compatible aliases configuration, if only one configuration is given
    pub fn preferred_protocol(&self) -> Option<&str> {
        if let Some(protocol) = self.preferred_protocol.as_deref() {
            return Some(protocol);
        }
        if self.protocol_configs.len() == 1 {
            return self.protocol_configs.keys().next().map(|x| x.as_str());
        } else if self.protocol_configs.is_empty() && self.oci_registry.is_some() {
            return Some(OCI_PROTOCOL);
        }
        None
    }

    /// Returns an iterator of protocols configured by the registry.
    pub fn configured_protocols(&self) -> impl Iterator<Item = Cow<'_, str>> {
        let mut protos: BTreeSet<String> = self.protocol_configs.keys().cloned().collect();
        // Backward-compatibility aliases
        if self.oci_registry.is_some() || self.oci_namespace_prefix.is_some() {
            protos.insert(OCI_PROTOCOL.into());
        }
        protos.into_iter().map(Into::into)
    }

    /// Deserializes protocol config for the given protocol.
    ///
    /// Returns `Ok(None)` if no configuration is available for the given
    /// protocol.
    /// Returns `Err` if configuration is available for the given protocol but
    /// deserialization fails.
    pub fn protocol_config<T: DeserializeOwned>(&self, protocol: &str) -> Result<Option<T>, Error> {
        let mut config = self.protocol_configs.get(protocol).cloned();

        // Backward-compatibility aliases
        let mut maybe_set = |key: &str, val: &Option<String>| {
            if let Some(value) = val {
                config
                    .get_or_insert_with(Default::default)
                    .insert(key.into(), value.clone().into());
            }
        };
        if protocol == OCI_PROTOCOL {
            maybe_set("registry", &self.oci_registry);
            maybe_set("namespacePrefix", &self.oci_namespace_prefix);
        }

        if config.is_none() {
            return Ok(None);
        }
        Ok(Some(
            serde_json::from_value(config.unwrap().into())
                .map_err(|err| Error::InvalidRegistryMetadata(err.into()))?,
        ))
    }

    /// Set the OCI registry
    #[cfg(feature = "oci_extras")]
    pub fn set_oci_registry(&mut self, registry: Option<String>) {
        self.oci_registry = registry;
    }

    /// Set the OCI namespace prefix
    #[cfg(feature = "oci_extras")]
    pub fn set_oci_namespace_prefix(&mut self, ns_prefix: Option<String>) {
        self.oci_namespace_prefix = ns_prefix;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[derive(Deserialize, Debug, PartialEq)]
    #[serde(rename_all = "camelCase")]
    struct OtherProtocolConfig {
        key: String,
    }

    #[test]
    fn smoke_test() {
        let meta: RegistryMetadata = serde_json::from_value(json!({
            "oci": {"registry": "oci.example.com"},
            "other": {"key": "value"}
        }))
        .unwrap();
        assert_eq!(meta.preferred_protocol(), None);
        assert_eq!(
            meta.configured_protocols().collect::<Vec<_>>(),
            ["oci", "other"]
        );
        let oci_config: JsonObject = meta.protocol_config("oci").unwrap().unwrap();
        assert_eq!(oci_config["registry"], "oci.example.com");
        let other_config: OtherProtocolConfig = meta.protocol_config("other").unwrap().unwrap();
        assert_eq!(other_config.key, "value");
    }

    #[test]
    fn preferred_protocol_explicit() {
        let meta: RegistryMetadata = serde_json::from_value(json!({
            "preferredProtocol": "oci",
            "oci": {"registry": "oci.example.com"},
            "other": {"key": "value"},
        }))
        .unwrap();
        assert_eq!(meta.preferred_protocol(), Some("oci"));
    }

    #[test]
    fn preferred_protocol_implicit_oci() {
        let meta: RegistryMetadata = serde_json::from_value(json!({
            "oci": {"registry": "oci.example.com"},
        }))
        .unwrap();
        assert_eq!(meta.preferred_protocol(), Some("oci"));
    }

    #[test]
    fn preferred_protocol_implicit_other() {
        let meta: RegistryMetadata = serde_json::from_value(json!({
            "other": {"key": "value"},
        }))
        .unwrap();
        assert_eq!(meta.preferred_protocol(), Some("other"));
    }

    #[test]
    fn backward_compat_preferred_protocol_implicit_oci() {
        let meta: RegistryMetadata = serde_json::from_value(json!({
            "ociRegistry": "oci.example.com",
            "ociNamespacePrefix": "prefix/",
        }))
        .unwrap();
        assert_eq!(meta.preferred_protocol(), Some("oci"));
    }

    #[test]
    fn basic_backward_compat_test() {
        let meta: RegistryMetadata = serde_json::from_value(json!({
            "ociRegistry": "oci.example.com",
            "ociNamespacePrefix": "prefix/",
        }))
        .unwrap();
        assert_eq!(meta.configured_protocols().collect::<Vec<_>>(), ["oci"]);
        let oci_config: JsonObject = meta.protocol_config("oci").unwrap().unwrap();
        assert_eq!(oci_config["registry"], "oci.example.com");
        assert_eq!(oci_config["namespacePrefix"], "prefix/");
    }

    #[test]
    fn merged_backward_compat_test() {
        let meta: RegistryMetadata = serde_json::from_value(json!({
            "ociRegistry": "oci.example.com",
            "other": {"key": "value"}
        }))
        .unwrap();
        assert_eq!(
            meta.configured_protocols().collect::<Vec<_>>(),
            ["oci", "other"]
        );
        let oci_config: JsonObject = meta.protocol_config("oci").unwrap().unwrap();
        assert_eq!(oci_config["registry"], "oci.example.com");
        let other_config: OtherProtocolConfig = meta.protocol_config("other").unwrap().unwrap();
        assert_eq!(other_config.key, "value");
    }

    #[test]
    fn bad_protocol_config() {
        let meta: RegistryMetadata = serde_json::from_value(json!({
            "other": {"bad": "config"}
        }))
        .unwrap();
        assert_eq!(meta.configured_protocols().collect::<Vec<_>>(), ["other"]);
        let res = meta.protocol_config::<OtherProtocolConfig>("other");
        assert!(res.is_err(), "{res:?}");
    }
}
