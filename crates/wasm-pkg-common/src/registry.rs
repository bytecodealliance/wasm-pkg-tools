use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap},
};

use http::uri::Authority;
use serde::{de::DeserializeOwned, Deserialize};

use crate::Error;

/// A registry identifier, which should be a valid HTTP Host.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Registry(Authority);

impl Registry {
    /// Returns the registry host, without port number.
    pub fn host(&self) -> &str {
        self.0.host()
    }

    /// Returns the registry port number, if given.
    pub fn port(&self) -> Option<u16> {
        self.0.port_u16()
    }
}

impl AsRef<str> for Registry {
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for Registry {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse()?))
    }
}

impl TryFrom<String> for Registry {
    type Error = Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Ok(Self(value.try_into()?))
    }
}

/// Well-Known URI (RFC 8615) path for registry metadata.
pub const REGISTRY_METADATA_PATH: &str = "/.well-known/wasm-pkg/registry.json";

type JsonObject = serde_json::Map<String, serde_json::Value>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryMetadata {
    /// The registry's preferred protocol.
    preferred_protocol: Option<String>,

    /// Protocol-specific configuration.
    #[serde(flatten)]
    protocol_configs: HashMap<String, JsonObject>,

    // Backward-compatibility aliases:
    /// OCI Registry
    oci_registry: Option<String>,
    /// OCI Namespace Prefix
    oci_namespace_prefix: Option<String>,
    /// Warg URL
    warg_url: Option<String>,
}

const OCI_PROTOCOL: &str = "oci";
const WARG_PROTOCOL: &str = "warg";

impl RegistryMetadata {
    /// Returns the registry's preferred protocol.
    ///
    /// The preferred protocol is:
    /// - the `preferredProtocol` metadata field, if given
    /// - the protocol configuration key, if only one configuration is given
    pub fn preferred_protocol(&self) -> Option<&str> {
        if let Some(protocol) = self.preferred_protocol.as_deref() {
            return Some(protocol);
        }
        if self.protocol_configs.len() == 1 {
            return self.protocol_configs.keys().next().map(|x| x.as_str());
        }
        None
    }

    /// Returns an iterator of protocols configured by the registry.
    pub fn configured_protocols(&self) -> impl Iterator<Item = Cow<str>> {
        let mut protos: BTreeSet<String> = self.protocol_configs.keys().cloned().collect();
        // Backward-compatibility aliases
        if self.oci_registry.is_some() || self.oci_namespace_prefix.is_some() {
            protos.insert(OCI_PROTOCOL.into());
        }
        if self.warg_url.is_some() {
            protos.insert(WARG_PROTOCOL.into());
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
        match protocol {
            OCI_PROTOCOL => {
                maybe_set("registry", &self.oci_registry);
                maybe_set("namespacePrefix", &self.oci_namespace_prefix);
            }
            WARG_PROTOCOL => {
                maybe_set("url", &self.warg_url);
            }
            _ => {}
        }

        if config.is_none() {
            return Ok(None);
        }
        Ok(Some(
            serde_json::from_value(config.unwrap().into())
                .map_err(Error::InvalidRegistryMetadata)?,
        ))
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
            "warg": {"url": "https://warg.example.com"},
        }))
        .unwrap();
        assert_eq!(meta.preferred_protocol(), None);
        assert_eq!(
            meta.configured_protocols().collect::<Vec<_>>(),
            ["oci", "warg"]
        );
        let oci_config: JsonObject = meta.protocol_config("oci").unwrap().unwrap();
        assert_eq!(oci_config["registry"], "oci.example.com");
        let warg_config: JsonObject = meta.protocol_config("warg").unwrap().unwrap();
        assert_eq!(warg_config["url"], "https://warg.example.com");
        let other_config: Option<OtherProtocolConfig> = meta.protocol_config("other").unwrap();
        assert_eq!(other_config, None);
    }

    #[test]
    fn preferred_protocol_explicit() {
        let meta: RegistryMetadata = serde_json::from_value(json!({
            "preferredProtocol": "warg",
            "oci": {"registry": "oci.example.com"},
            "warg": {"url": "https://warg.example.com"},
        }))
        .unwrap();
        assert_eq!(meta.preferred_protocol(), Some("warg"));
    }

    #[test]
    fn preferred_protocol_implicit() {
        let meta: RegistryMetadata = serde_json::from_value(json!({
            "oci": {"registry": "oci.example.com"},
        }))
        .unwrap();
        assert_eq!(meta.preferred_protocol(), Some("oci"));
    }

    #[test]
    fn basic_backward_compat_test() {
        let meta: RegistryMetadata = serde_json::from_value(json!({
            "ociRegistry": "oci.example.com",
            "ociNamespacePrefix": "prefix/",
            "wargUrl": "https://warg.example.com",
        }))
        .unwrap();
        assert_eq!(
            meta.configured_protocols().collect::<Vec<_>>(),
            ["oci", "warg"]
        );
        let oci_config: JsonObject = meta.protocol_config("oci").unwrap().unwrap();
        assert_eq!(oci_config["registry"], "oci.example.com");
        assert_eq!(oci_config["namespacePrefix"], "prefix/");
        let warg_config: JsonObject = meta.protocol_config("warg").unwrap().unwrap();
        assert_eq!(warg_config["url"], "https://warg.example.com");
    }

    #[test]
    fn merged_backward_compat_test() {
        let meta: RegistryMetadata = serde_json::from_value(json!({
            "wargUrl": "https://warg.example.com",
            "other": {"key": "value"}
        }))
        .unwrap();
        assert_eq!(
            meta.configured_protocols().collect::<Vec<_>>(),
            ["other", "warg"]
        );
        let warg_config: JsonObject = meta.protocol_config("warg").unwrap().unwrap();
        assert_eq!(warg_config["url"], "https://warg.example.com");
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
