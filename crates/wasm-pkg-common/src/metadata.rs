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
    /// Warg URL
    #[serde(skip_serializing)]
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
    /// - the protocol backward-compatible aliases configuration, if only one configuration is given
    pub fn preferred_protocol(&self) -> Option<&str> {
        if let Some(protocol) = self.preferred_protocol.as_deref() {
            return Some(protocol);
        }
        if self.protocol_configs.len() == 1 {
            return self.protocol_configs.keys().next().map(|x| x.as_str());
        } else if self.protocol_configs.is_empty() {
            match (self.oci_registry.is_some(), self.warg_url.is_some()) {
                (true, false) => return Some(OCI_PROTOCOL),
                (false, true) => return Some(WARG_PROTOCOL),
                _ => {}
            }
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
                .map_err(|err| Error::InvalidRegistryMetadata(err.into()))?,
        ))
    }
}

#[cfg(feature = "metadata-client")]
mod client {
    use anyhow::Context;
    use http::StatusCode;

    use super::REGISTRY_METADATA_PATH;
    use crate::{registry::Registry, Error};

    impl super::RegistryMetadata {
        pub async fn fetch_or_default(registry: &Registry) -> Self {
            match Self::fetch(registry).await {
                Ok(Some(meta)) => {
                    tracing::debug!(?meta, "Got registry metadata");
                    meta
                }
                Ok(None) => {
                    tracing::debug!("Metadata not found");
                    Default::default()
                }
                Err(err) => {
                    tracing::warn!(error = ?err, "Error fetching registry metadata");
                    Default::default()
                }
            }
        }

        pub async fn fetch(registry: &Registry) -> Result<Option<Self>, Error> {
            let scheme = if registry.host() == "localhost" {
                "http"
            } else {
                "https"
            };
            let url = format!("{scheme}://{registry}{REGISTRY_METADATA_PATH}");
            Self::fetch_url(&url)
                .await
                .with_context(|| format!("error fetching registry metadata from {url:?}"))
                .map_err(Error::RegistryMetadataError)
        }

        async fn fetch_url(url: &str) -> anyhow::Result<Option<Self>> {
            tracing::debug!(?url, "Fetching registry metadata");

            let resp = reqwest::get(url).await?;
            if resp.status() == StatusCode::NOT_FOUND {
                return Ok(None);
            }
            let resp = resp.error_for_status()?;
            Ok(Some(resp.json().await?))
        }
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
    fn preferred_protocol_implicit_oci() {
        let meta: RegistryMetadata = serde_json::from_value(json!({
            "oci": {"registry": "oci.example.com"},
        }))
        .unwrap();
        assert_eq!(meta.preferred_protocol(), Some("oci"));
    }

    #[test]
    fn preferred_protocol_implicit_warg() {
        let meta: RegistryMetadata = serde_json::from_value(json!({
            "warg": {"url": "https://warg.example.com"},
        }))
        .unwrap();
        assert_eq!(meta.preferred_protocol(), Some("warg"));
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
    fn backward_compat_preferred_protocol_implicit_warg() {
        let meta: RegistryMetadata = serde_json::from_value(json!({
            "wargUrl": "https://warg.example.com",
        }))
        .unwrap();
        assert_eq!(meta.preferred_protocol(), Some("warg"));
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
