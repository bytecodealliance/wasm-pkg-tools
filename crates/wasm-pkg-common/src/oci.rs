//! Client and configuration for interacting with an OCI registry.

use std::{collections::HashMap, hash::Hash, ops::Deref, sync::Arc};

use docker_credential::{CredentialRetrievalError, DockerCredential};
use oci_distribution::{
    client::{Client, ClientConfig},
    secrets::RegistryAuth,
    Reference, RegistryOperation,
};
use oci_wasm::WasmClient;
use secrecy::ExposeSecret;
use tokio::sync::RwLock;

use crate::{config::oci::BasicCredentials, Error};

/// Configuration for the OCI client.
#[derive(Default)]
pub struct OciConfig {
    pub client_config: ClientConfig,
    pub credentials: Option<BasicCredentials>,
}

impl Clone for OciConfig {
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

impl std::fmt::Debug for OciConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OciConfig")
            .field("client_config", &"...")
            .field("credentials", &self.credentials)
            .finish()
    }
}

#[derive(PartialEq, Eq)]
struct AuthKey {
    registry: String,
    operation: RegistryOperation,
}

impl Hash for AuthKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.registry.hash(state);
        // Operation isn't hash, so we're faking it here
        match self.operation {
            RegistryOperation::Pull => 0usize.hash(state),
            RegistryOperation::Push => 1usize.hash(state),
        }
    }
}

/// Common client for an OCI registry
pub struct Oci {
    client: WasmClient,
    registry_auth: Arc<RwLock<HashMap<AuthKey, RegistryAuth>>>,
    credentials: Option<BasicCredentials>,
}

impl AsRef<WasmClient> for Oci {
    fn as_ref(&self) -> &WasmClient {
        &self.client
    }
}

impl Deref for Oci {
    type Target = WasmClient;
    fn deref(&self) -> &Self::Target {
        &self.client
    }
}

impl AsRef<Client> for Oci {
    fn as_ref(&self) -> &Client {
        self.client.as_ref()
    }
}

impl Oci {
    /// Create a new OCI client using the given config
    pub fn new(config: OciConfig) -> Self {
        let OciConfig {
            client_config,
            credentials,
        } = config;
        let client = Client::new(client_config);
        let client = WasmClient::new(client);

        Self {
            client,
            registry_auth: Arc::default(),
            credentials,
        }
    }

    /// Returns the configured registry authentication type for the given reference and operation,
    /// authenticating if needed. Only returns an error if the client cannot be authenticated
    pub async fn get_auth(
        &self,
        reference: &Reference,
        operation: RegistryOperation,
    ) -> Result<RegistryAuth, Error> {
        let key = AuthKey {
            registry: reference.resolve_registry().to_owned(),
            operation,
        };
        // Take a read lock on the data first to avoid blocking on the write lock
        {
            let registry_auth = self.registry_auth.read().await;
            if let Some(auth) = registry_auth.get(&key) {
                return Ok(auth.clone());
            }
        }
        let credentials = self.credentials.clone();
        let ref_clone = reference.clone();
        let mut auth =
            tokio::task::spawn_blocking(move || get_credentials(credentials, &ref_clone))
                .await
                .map_err(|e| {
                    Error::CredentialError(anyhow::anyhow!(
                        "Unable to await credential retrieval: {}",
                        e
                    ))
                })??;
        // Preflight auth to check for validity; this isn't wasted effort because the
        // oci_distribution::Client caches it
        use oci_distribution::errors::OciDistributionError::AuthenticationFailure;
        use oci_distribution::RegistryOperation::Pull;
        match self.client.auth(reference, &auth, key.operation).await {
            Ok(_) => (),
            Err(err @ AuthenticationFailure(_)) if auth != RegistryAuth::Anonymous => {
                // The failed credentials might not even be required for this image; retry anonymously
                if self
                    .client
                    .auth(reference, &RegistryAuth::Anonymous, Pull)
                    .await
                    .is_ok()
                {
                    auth = RegistryAuth::Anonymous;
                } else {
                    return Err(Error::CredentialError(err.into()));
                }
            }
            Err(err) => return Err(Error::CredentialError(err.into())),
        }
        // Now we have a valid auth, write it to the map
        let mut registry_auth = self.registry_auth.write().await;
        registry_auth.insert(key, auth.clone());
        Ok(auth)
    }
}

fn get_credentials(
    credentials: Option<BasicCredentials>,
    reference: &Reference,
) -> Result<RegistryAuth, Error> {
    if let Some(BasicCredentials { username, password }) = credentials {
        return Ok(RegistryAuth::Basic(
            username,
            password.expose_secret().to_owned(),
        ));
    }

    let server_url = format!("https://{}", reference.resolve_registry());
    match docker_credential::get_credential(&server_url) {
        Ok(DockerCredential::UsernamePassword(username, password)) => {
            return Ok(RegistryAuth::Basic(username, password));
        }
        Ok(DockerCredential::IdentityToken(_)) => {
            return Err(Error::CredentialError(anyhow::anyhow!(
                "identity tokens not supported"
            )));
        }
        Err(err) => {
            if matches!(
                err,
                CredentialRetrievalError::ConfigNotFound
                    | CredentialRetrievalError::ConfigReadError
                    | CredentialRetrievalError::NoCredentialConfigured
            ) {
                tracing::debug!(?err, "Failed to look up OCI credentials");
            } else {
                tracing::warn!(
                    ?err,
                    "Failed to look up OCI credentials, falling back to anonymous auth"
                );
            };
        }
    }

    Ok(RegistryAuth::Anonymous)
}
