use anyhow::Context;
use base64::{
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
    Engine,
};
use oci_distribution::client::ClientProtocol;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use super::serialize_string_secret;
use crate::{oci::OciConfig, Error};

// Configuration for a specific registry
pub struct OciRegistryConfig {
    pub auth: Option<BasicCredentials>,
    pub protocol: Option<ClientProtocol>,
}

impl From<OciRegistryConfig> for OciConfig {
    fn from(value: OciRegistryConfig) -> Self {
        let mut client_config = oci_distribution::client::ClientConfig::default();
        if let Some(protocol) = value.protocol {
            client_config.protocol = protocol;
        };
        OciConfig {
            client_config,
            credentials: value.auth,
        }
    }
}

#[derive(Deserialize, Serialize)]
struct OciRegistryConfigIntermediate {
    auth: Option<TomlAuth>,
    protocol: Option<String>,
}

impl<'de> Deserialize<'de> for OciRegistryConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let intermediate = OciRegistryConfigIntermediate::deserialize(deserializer)?;
        let auth = intermediate
            .auth
            .map(TryInto::try_into)
            .transpose()
            .map_err(serde::de::Error::custom)?;

        Ok(OciRegistryConfig {
            auth,
            protocol: intermediate
                .protocol
                .as_deref()
                .map(oci_client_protocol)
                .transpose()
                .map_err(serde::de::Error::custom)?,
        })
    }
}

impl Serialize for OciRegistryConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let intermediate = OciRegistryConfigIntermediate {
            auth: self.auth.as_ref().map(|auth| TomlAuth::UsernamePassword {
                username: auth.username.clone(),
                password: auth.password.clone(),
            }),
            protocol: self.protocol.as_ref().map(|protocol| {
                match protocol {
                    ClientProtocol::Http => "http".to_string(),
                    // NOTE(thomastaylor312): We can't really convert https except, but we are only
                    // implementing serialize so we can manually store a config.
                    ClientProtocol::Https | ClientProtocol::HttpsExcept(_) => "https".to_string(),
                }
            }),
        };
        intermediate.serialize(serializer)
    }
}

/// Basic Credentials for registry authentication
#[derive(Clone, Debug)]
pub struct BasicCredentials {
    pub username: String,
    pub password: SecretString,
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
#[serde(deny_unknown_fields)]
enum TomlAuth {
    Base64(#[serde(serialize_with = "serialize_string_secret")] SecretString),
    UsernamePassword {
        username: String,
        #[serde(serialize_with = "serialize_string_secret")]
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
                let bs = OCI_AUTH_BASE64.decode(b64.expose_secret())?;
                let s = String::from_utf8(bs)?;
                let (username, password) = s
                    .split_once(':')
                    .context("expected <username>:<password> but no ':' found")?;
                Ok(BasicCredentials {
                    username: username.into(),
                    password: password.to_string().into(),
                })
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
        _ => Err(Error::InvalidConfig(
            anyhow::anyhow!("Unknown OCI protocol {text:?}").into(),
        )),
    }
}
