use std::path::PathBuf;

use secrecy::SecretString;
use serde::Deserialize;
use wasm_pkg_common::{config::RegistryConfig, Error};

#[derive(Clone, Debug, Default)]
pub struct WargConfig {
    pub client_config: Option<warg_client::Config>,
    pub auth_token: Option<SecretString>,
}

impl TryFrom<&RegistryConfig> for WargConfig {
    type Error = Error;

    fn try_from(registry_config: &RegistryConfig) -> Result<Self, Self::Error> {
        let WargRegistryConfigToml {
            auth_token,
            config_file,
        } = registry_config.backend_config("warg")?.unwrap_or_default();
        let client_config = match config_file {
            Some(path) => Some(warg_client::Config::from_file(path).map_err(Error::RegistryError)?),
            None => Some(
                warg_client::Config::from_default_file()
                    .map_err(Error::RegistryError)?
                    .unwrap_or_default(),
            ),
        };
        Ok(Self {
            client_config,
            auth_token,
        })
    }
}

#[derive(Default, Deserialize)]
struct WargRegistryConfigToml {
    config_file: Option<PathBuf>,
    auth_token: Option<SecretString>,
}
