use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::Context;
use base64::{
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
    Engine,
};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use crate::{
    source::{local::LocalConfig, oci::OciConfig, warg::WargConfig},
    Error,
};

use super::BasicCredentials;

impl super::ClientConfig {
    pub fn from_toml(s: &str) -> Result<Self, Error> {
        let toml_cfg: TomlConfig = toml::from_str(s)
            .context("error parsing TOML")
            .map_err(Error::InvalidConfig)?;
        toml_cfg.try_into().map_err(Error::InvalidConfig)
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, Error> {
        tracing::debug!("Reading config file from {:?}", path.as_ref());
        Self::from_toml(std::fs::read_to_string(path)?.as_str())
    }

    pub fn from_default_file() -> Result<Option<Self>, Error> {
        let Some(config_dir) = dirs::config_dir() else {
            return Ok(None);
        };
        let path = config_dir.join("warg").join("config.toml");
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(Self::from_file(path)?))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlConfig {
    default_registry: Option<String>,
    #[serde(default)]
    namespace: HashMap<String, TomlNamespaceConfig>,
    #[serde(default)]
    registry: HashMap<String, TomlRegistryConfig>,
}

impl TryFrom<TomlConfig> for super::ClientConfig {
    type Error = anyhow::Error;

    fn try_from(value: TomlConfig) -> Result<Self, Self::Error> {
        let TomlConfig {
            default_registry,
            namespace,
            registry,
        } = value;
        let namespace_registries = namespace
            .into_iter()
            .map(|(name, config)| (name, config.registry))
            .collect();
        let registry_configs = registry
            .into_iter()
            .map(|(k, v)| Ok((k, v.try_into()?)))
            .collect::<Result<_, Self::Error>>()?;
        Ok(Self {
            default_registry,
            namespace_registries,
            registry_configs,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlNamespaceConfig {
    registry: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
enum TomlRegistryConfig {
    Local {
        root: PathBuf,
    },
    Oci {
        auth: Option<TomlAuth>,
    },
    Warg {
        auth_token: Option<SecretString>,
        config_file: Option<PathBuf>,
    },
}

impl TryFrom<TomlRegistryConfig> for super::RegistryConfig {
    type Error = anyhow::Error;

    fn try_from(value: TomlRegistryConfig) -> Result<Self, Self::Error> {
        Ok(match value {
            TomlRegistryConfig::Local { root } => Self::Local(LocalConfig { root }),
            TomlRegistryConfig::Oci { auth } => {
                let credentials = auth.map(TryInto::try_into).transpose()?;
                Self::Oci(OciConfig {
                    client_config: None,
                    credentials,
                })
            }
            TomlRegistryConfig::Warg {
                auth_token,
                config_file,
            } => {
                let client_config = match config_file {
                    Some(path) => warg_client::Config::from_file(path)?,
                    None => warg_client::Config::from_default_file()?.unwrap_or_default(),
                };
                Self::Warg(WargConfig {
                    auth_token,
                    client_config,
                })
            }
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
#[serde(deny_unknown_fields)]
enum TomlAuth {
    Base64(SecretString),
    UsernamePassword {
        username: String,
        password: SecretString,
    },
}

const OCI_AUTH_BASE64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

impl TryFrom<TomlAuth> for BasicCredentials {
    type Error = anyhow::Error;

    fn try_from(value: TomlAuth) -> Result<Self, Self::Error> {
        match value {
            TomlAuth::Base64(b64) => {
                fn decode_b64_creds(b64: &str) -> anyhow::Result<BasicCredentials> {
                    let bs = OCI_AUTH_BASE64.decode(b64)?;
                    let s = String::from_utf8(bs)?;
                    let (username, password) = s
                        .split_once(':')
                        .context("expected <username>:<password> but no ':' found")?;
                    Ok(BasicCredentials {
                        username: username.into(),
                        password: password.to_string().into(),
                    })
                }
                decode_b64_creds(b64.expose_secret()).context("invalid base64-encoded creds")
            }
            TomlAuth::UsernamePassword { username, password } => {
                Ok(BasicCredentials { username, password })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{ClientConfig, RegistryConfig};

    use super::*;

    #[test]
    fn smoke_test() {
        let toml_config = r#"
            default_registry = "example.com"

            [namespace.wasi]
            registry = "wasi.dev"

            [registry."example.com"]
            type = "oci"
            auth = { username = "open", password = "sesame" }

            [registry."wasi.dev"]
            type = "oci"
            auth = "cGluZzpwb25n"
        "#;
        let cfg = ClientConfig::from_toml(toml_config).unwrap();

        assert_eq!(cfg.default_registry.as_deref(), Some("example.com"));
        assert_eq!(cfg.namespace_registries["wasi"], "wasi.dev");

        let RegistryConfig::Oci(oci_config) = &cfg.registry_configs["example.com"] else {
            panic!("not an oci config");
        };
        let BasicCredentials { username, password } = oci_config.credentials.as_ref().unwrap();
        assert_eq!(username, "open");
        assert_eq!(password.expose_secret(), "sesame");

        let RegistryConfig::Oci(oci_config) = &cfg.registry_configs["wasi.dev"] else {
            panic!("not an oci config");
        };
        let BasicCredentials { username, password } = oci_config.credentials.as_ref().unwrap();
        assert_eq!(username, "ping");
        assert_eq!(password.expose_secret(), "pong");
    }
}
