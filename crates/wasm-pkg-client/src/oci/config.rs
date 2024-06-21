use anyhow::Context;
use base64::{
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
    Engine,
};
use oci_distribution::client::ClientConfig;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use wasm_pkg_common::{config::RegistryConfig, Error};

/// Registry configuration for OCI backends.
///
/// See: [`RegistryConfig::backend_config`]
#[derive(Default)]
pub struct OciRegistryConfig {
    pub client_config: ClientConfig,
    pub credentials: Option<BasicCredentials>,
}

impl Clone for OciRegistryConfig {
    fn clone(&self) -> Self {
        let client_config = ClientConfig {
            protocol: self.client_config.protocol.clone(),
            extra_root_certificates: self.client_config.extra_root_certificates.clone(),
            platform_resolver: None,
            ..self.client_config
        };
        Self {
            client_config,
            credentials: self.credentials.clone(),
        }
    }
}

impl std::fmt::Debug for OciRegistryConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OciConfig")
            .field("client_config", &"...")
            .field("credentials", &self.credentials)
            .finish()
    }
}

impl TryFrom<&RegistryConfig> for OciRegistryConfig {
    type Error = Error;

    fn try_from(registry_config: &RegistryConfig) -> Result<Self, Self::Error> {
        let OciRegistryConfigToml { auth, protocol } =
            registry_config.backend_config("oci")?.unwrap_or_default();
        let mut client_config = ClientConfig::default();
        if let Some(protocol) = protocol {
            client_config.protocol = oci_client_protocol(&protocol)?;
        };
        let credentials = auth
            .map(TryInto::try_into)
            .transpose()
            .map_err(Error::InvalidConfig)?;
        Ok(Self {
            client_config,
            credentials,
        })
    }
}

#[derive(Default, Deserialize)]
struct OciRegistryConfigToml {
    auth: Option<TomlAuth>,
    protocol: Option<String>,
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

#[derive(Clone, Debug)]
pub struct BasicCredentials {
    pub username: String,
    pub password: SecretString,
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

fn oci_client_protocol(text: &str) -> Result<oci_distribution::client::ClientProtocol, Error> {
    match text {
        "http" => Ok(oci_distribution::client::ClientProtocol::Http),
        "https" => Ok(oci_distribution::client::ClientProtocol::Https),
        _ => Err(Error::InvalidConfig(anyhow::anyhow!(
            "Unknown OCI protocol {text:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_test() {
        let toml_config = r#"
            [registry."example.com"]
            type = "oci"
            [registry."example.com".oci]
            auth = { username = "open", password = "sesame" }
            protocol = "http"

            [registry."wasi.dev"]
            type = "oci"
            [registry."wasi.dev".oci]
            auth = "cGluZzpwb25n"
        "#;
        let cfg = wasm_pkg_common::config::Config::from_toml(toml_config).unwrap();

        let oci_config: OciRegistryConfig = cfg
            .registry_config(&"example.com".parse().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let BasicCredentials { username, password } = oci_config.credentials.as_ref().unwrap();
        assert_eq!(username, "open");
        assert_eq!(password.expose_secret(), "sesame");
        assert_eq!(
            oci_distribution::client::ClientProtocol::Http,
            oci_config.client_config.protocol
        );

        let oci_config: OciRegistryConfig = cfg
            .registry_config(&"wasi.dev".parse().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let BasicCredentials { username, password } = oci_config.credentials.as_ref().unwrap();
        assert_eq!(username, "ping");
        assert_eq!(password.expose_secret(), "pong");
    }
}
