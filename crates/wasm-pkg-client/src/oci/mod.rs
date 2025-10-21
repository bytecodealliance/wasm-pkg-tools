//! OCI package client.
//!
//! This follows the CNCF TAG Runtime guidance for [Wasm OCI Artifacts][1].
//!
//! [1]: https://tag-runtime.cncf.io/wgs/wasm/deliverables/wasm-oci-artifact/

mod config;
mod loader;
mod publisher;

use docker_credential::{CredentialRetrievalError, DockerCredential};
use oci_client::{
    errors::OciDistributionError, secrets::RegistryAuth, Reference, RegistryOperation,
};
use secrecy::ExposeSecret;
use serde::Deserialize;
use tokio::sync::OnceCell;
use wasm_pkg_common::{
    config::RegistryConfig,
    metadata::RegistryMetadata,
    package::{PackageRef, Version},
    registry::Registry,
    Error,
};

/// Re-exported for convenience.
pub use oci_client::client;

pub use config::{BasicCredentials, OciRegistryConfig};

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OciRegistryMetadata {
    registry: Option<String>,
    namespace_prefix: Option<String>,
}

pub(crate) struct OciBackend {
    client: oci_wasm::WasmClient,
    oci_registry: String,
    namespace_prefix: Option<String>,
    credentials: Option<BasicCredentials>,
    registry_auth: OnceCell<RegistryAuth>,
}

impl OciBackend {
    pub fn new(
        registry: &Registry,
        registry_config: &RegistryConfig,
        registry_meta: &RegistryMetadata,
    ) -> Result<Self, Error> {
        let OciRegistryConfig {
            client_config,
            credentials,
        } = registry_config.try_into()?;
        let client = oci_client::Client::new(client_config);
        let client = oci_wasm::WasmClient::new(client);

        let oci_meta = registry_meta
            .protocol_config::<OciRegistryMetadata>("oci")?
            .unwrap_or_default();
        let oci_registry = oci_meta.registry.unwrap_or_else(|| registry.to_string());

        Ok(Self {
            client,
            oci_registry,
            namespace_prefix: oci_meta.namespace_prefix,
            credentials,
            registry_auth: OnceCell::new(),
        })
    }

    pub(crate) async fn auth(
        &self,
        reference: &Reference,
        operation: RegistryOperation,
    ) -> Result<RegistryAuth, Error> {
        self.registry_auth
            .get_or_try_init(|| async {
                let mut auth = self.get_credentials()?;
                // Preflight auth to check for validity; this isn't wasted
                // effort because the oci_client::Client caches it
                use oci_client::errors::OciDistributionError::AuthenticationFailure;
                match self.client.auth(reference, &auth, operation).await {
                    Ok(_) => (),
                    Err(err @ AuthenticationFailure(_)) if auth != RegistryAuth::Anonymous => {
                        // The failed credentials might not even be required for this image; retry anonymously
                        if self
                            .client
                            .auth(reference, &RegistryAuth::Anonymous, operation)
                            .await
                            .is_ok()
                        {
                            auth = RegistryAuth::Anonymous;
                        } else {
                            return Err(oci_registry_error(err));
                        }
                    }
                    Err(err) => return Err(oci_registry_error(err)),
                }
                Ok(auth)
            })
            .await
            .cloned()
    }

    pub(crate) fn get_credentials(&self) -> Result<RegistryAuth, Error> {
        if let Some(BasicCredentials { username, password }) = &self.credentials {
            return Ok(RegistryAuth::Basic(
                username.clone(),
                password.expose_secret().clone(),
            ));
        }

        match docker_credential::get_credential(&self.oci_registry) {
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
                        | CredentialRetrievalError::HelperFailure { .. }
                ) {
                    tracing::debug!("Failed to look up OCI credentials: {err}");
                } else {
                    tracing::warn!("Failed to look up OCI credentials: {err}");
                };
            }
        }

        Ok(RegistryAuth::Anonymous)
    }

    pub(crate) fn make_reference(
        &self,
        package: &PackageRef,
        version: Option<&Version>,
    ) -> Reference {
        let repository = format!(
            "{}{}/{}",
            self.namespace_prefix.as_deref().unwrap_or_default(),
            package.namespace(),
            package.name()
        );
        let tag = version
            .map(|ver| ver.to_string())
            .unwrap_or_else(|| "latest".into());
        Reference::with_tag(self.oci_registry.clone(), repository, tag)
    }
}

pub(crate) fn oci_registry_error(err: OciDistributionError) -> Error {
    match err {
        // Technically this could be a missing version too, but there really isn't a way to find out
        OciDistributionError::ImageManifestNotFoundError(_) => Error::PackageNotFound,
        _ => Error::RegistryError(err.into()),
    }
}
