//! Contains a structured client config type and helpers for Warg
use std::path::PathBuf;

use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use wasm_pkg_common::Error;

use super::serialize_option_string_secret;

/// The raw config type for the Warg registry. Can be turned into a [`WargConfig`].
#[derive(Deserialize, Serialize)]
pub struct WargRawConfig {
    #[serde(serialize_with = "serialize_option_string_secret")]
    pub auth_token: Option<SecretString>,
    pub config_file: Option<PathBuf>,
}

pub struct WargRegistryConfig {
    pub auth_token: Option<SecretString>,
    pub client_config: warg_client::Config,
}

impl TryFrom<WargRawConfig> for WargRegistryConfig {
    type Error = Error;

    fn try_from(value: WargRawConfig) -> Result<Self, Self::Error> {
        let client_config = match value.config_file {
            Some(path) => {
                warg_client::Config::from_file(path).map_err(|e| Error::InvalidConfig(e.into()))?
            }
            None => warg_client::Config::from_default_file()
                .map_err(|e| Error::InvalidConfig(e.into()))?
                .unwrap_or_default(),
        };
        Ok(WargRegistryConfig {
            auth_token: value.auth_token,
            client_config,
        })
    }
}
