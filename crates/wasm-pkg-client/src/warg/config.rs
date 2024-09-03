use std::{fmt::Debug, path::PathBuf, sync::Arc};

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, Serializer};
use warg_crypto::signing::PrivateKey;
use wasm_pkg_common::{config::RegistryConfig, Error};

/// Registry configuration for Warg backends.
///
/// See: [`RegistryConfig::backend_config`]
#[derive(Clone, Default, Serialize)]
#[serde(into = "WargRegistryConfigToml")]
pub struct WargRegistryConfig {
    /// The configuration for the Warg client.
    pub client_config: warg_client::Config,
    /// The authentication token for the Warg registry.
    pub auth_token: Option<SecretString>,
    /// A signing key to use for publishing packages.
    // NOTE(thomastaylor312): This couldn't be wrapped in a secret because the outer type doesn't
    // implement Zeroize. However, the inner type is zeroized.
    pub signing_key: Option<Arc<PrivateKey>>,
    /// The path to the Warg config file, if specified.
    pub config_file: Option<PathBuf>,
}

impl Debug for WargRegistryConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WargRegistryConfig")
            .field("client_config", &self.client_config)
            .field("auth_token", &self.auth_token)
            .field("signing_key", &"[redacted]")
            .field("config_file", &self.config_file)
            .finish()
    }
}

impl TryFrom<&RegistryConfig> for WargRegistryConfig {
    type Error = Error;

    fn try_from(registry_config: &RegistryConfig) -> Result<Self, Self::Error> {
        let WargRegistryConfigToml {
            auth_token,
            signing_key,
            config_file,
        } = registry_config.backend_config("warg")?.unwrap_or_default();
        let (client_config, config_file) = match config_file {
            Some(path) => (
                warg_client::Config::from_file(&path).map_err(Error::RegistryError)?,
                Some(path),
            ),
            None => {
                // NOTE(thomastaylor312): We could try to be smarter here and see which file it
                // loaded, but there isn't a way to do that if it loaded from the current working
                // directory.
                (
                    warg_client::Config::from_default_file()
                        .map_err(Error::RegistryError)?
                        .unwrap_or_default(),
                    None,
                )
            }
        };

        Ok(Self {
            client_config,
            auth_token,
            signing_key: signing_key
                .map(|k| PrivateKey::decode(k).map(Arc::new))
                .transpose()
                .map_err(|e| {
                    Error::InvalidConfig(anyhow::anyhow!("invalid signing key in config file: {e}"))
                })?,
            config_file,
        })
    }
}

#[derive(Default, Deserialize, Serialize)]
struct WargRegistryConfigToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    config_file: Option<PathBuf>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_secret"
    )]
    auth_token: Option<SecretString>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_secret"
    )]
    signing_key: Option<SecretString>,
}

impl From<WargRegistryConfig> for WargRegistryConfigToml {
    fn from(value: WargRegistryConfig) -> Self {
        WargRegistryConfigToml {
            auth_token: value.auth_token,
            config_file: value.config_file,
            signing_key: value
                .signing_key
                .map(|k| SecretString::new(k.encode().to_string())),
        }
    }
}

fn serialize_secret<S: Serializer>(
    secret: &Option<SecretString>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if let Some(secret) = secret {
        secret.expose_secret().serialize(serializer)
    } else {
        serializer.serialize_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_warg_config_roundtrip() {
        let dir = tempfile::tempdir().expect("Unable to create tempdir");
        let warg_config_path = dir.path().join("warg_config.json");
        let (_, key) = warg_crypto::signing::generate_p256_pair();
        let config = WargRegistryConfig {
            client_config: warg_client::Config {
                home_url: Some("https://example.com".to_owned()),
                ..Default::default()
            },
            auth_token: Some("imsecret".to_owned().into()),
            signing_key: Some(Arc::new(key)),
            config_file: Some(warg_config_path.clone()),
        };

        // Try loading it with the normal method to make sure it comes out right
        let mut conf = crate::Config::empty();

        let registry: crate::Registry = "example.com:8080".parse().unwrap();
        let reg_conf = conf.get_or_insert_registry_config_mut(&registry);
        reg_conf
            .set_backend_config("warg", &config)
            .expect("Unable to set config");

        let reg_conf = conf.registry_config(&registry).unwrap();

        // Write the warg config to disk
        tokio::fs::write(
            &warg_config_path,
            serde_json::to_vec(&config.client_config).unwrap(),
        )
        .await
        .unwrap();

        let roundtripped = WargRegistryConfig::try_from(reg_conf).expect("Unable to load config");
        assert_eq!(
            roundtripped
                .client_config
                .home_url
                .expect("Should have a home url set"),
            config.client_config.home_url.unwrap(),
            "Home url should be set to the right value"
        );
        assert_eq!(
            roundtripped
                .auth_token
                .expect("Should have an auth token set")
                .expose_secret(),
            config.auth_token.unwrap().expose_secret(),
            "Auth token should be set to the right value"
        );
        assert_eq!(
            roundtripped
                .signing_key
                .expect("Should have a signing key set")
                .encode(),
            config.signing_key.unwrap().encode(),
            "Signing key should be set to the right value"
        );
    }
}
