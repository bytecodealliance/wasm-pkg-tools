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
    Reference, RegistryOperation,
    errors::{OciDistributionError, OciError, OciErrorCode},
    secrets::RegistryAuth,
};
use secrecy::ExposeSecret;
use serde::Deserialize;
use tokio::sync::OnceCell;
use wasm_pkg_common::{
    Error,
    config::RegistryConfig,
    metadata::RegistryMetadata,
    package::{PackageRef, Version},
    registry::Registry,
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
            .map_err(|e| {
                if let Error::RegistryError(anyhow_err) = e {
                    Error::RegistryError(anyhow_err.context(reference.repository().to_owned()))
                } else {
                    e
                }
            })
            .cloned()
    }

    pub(crate) fn get_credentials(&self) -> Result<RegistryAuth, Error> {
        // Detect `WKG_REGISTRY_<REGISTRY>_AUTH_<AUTH_SCHEME>` if present.
        let auth_var_key = &registry_auth_env_var(&self.oci_registry, "BEARER");
        if let Ok(token) = std::env::var(auth_var_key)
            && !token.is_empty()
        {
            tracing::debug!(registry = %self.oci_registry, %auth_var_key, "Using detected authentication envvar key");
            // Only `BEARER` AUTH_SCHEME for now
            return Ok(RegistryAuth::Bearer(token));
        }

        if let Some(BasicCredentials { username, password }) = &self.credentials {
            return Ok(RegistryAuth::Basic(
                username.clone(),
                password.expose_secret().clone(),
            ));
        }

        match get_docker_credential(&self.oci_registry)? {
            Some(c) => Ok(c),
            None => {
                tracing::debug!("Failed to look up OCI credentials by registry, trying server URL");
                let server_url = format!("https://{}", self.oci_registry);
                match get_docker_credential(&server_url)? {
                    Some(c) => Ok(c),
                    None => Ok(RegistryAuth::Anonymous),
                }
            }
        }
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
        // `list_tags` against a repository that doesn't yet exist surfaces
        // as a `NameUnknown` envelope rather than a 404 manifest error. Only
        // Cast when NameUnknown is the *sole* error in the envelope.
        // Bundled errors (e.g. `[NameUnknown, Unauthorized]`) are preserved as a
        // generic `RegistryError`.
        OciDistributionError::RegistryError { ref envelope, .. }
            if matches!(
                envelope.errors.as_slice(),
                [OciError {
                    code: OciErrorCode::NameUnknown,
                    ..
                }],
            ) =>
        {
            Error::PackageNotFound
        }
        _ => Error::RegistryError(err.into()),
    }
}

/// Returns the envvar used to provide credentials for a given registry and
/// auth scheme: `WKG_REGISTRY_<REGISTRY>_AUTH_<AUTH_SCHEME>`
/// * `<REGISTRY>` : defined by the resolution of [`wasm_pkg_common::config::Config`] and
/// * `<AUTH_SCHEME>`: the auth mechanism (`BEARER` only support)
///
/// Any non-ASCII-alphanumeric characters (., -, :, /, ...) in the registry
/// name are replaced by `_` and the whole string is upper-cased.
// TODO(mkatychev): move this into `wasm_pkg_common::config::Config` once overlays are implemented,
// this should be a generic-to-backend way of overriding configs.
fn registry_auth_env_var(oci_registry: &str, scheme: &str) -> String {
    let sanitized: String = oci_registry
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!(
        "WKG_REGISTRY_{}_AUTH_{}",
        sanitized.to_ascii_uppercase(),
        scheme.to_ascii_uppercase(),
    )
}

fn get_docker_credential(registry: &str) -> Result<Option<RegistryAuth>, Error> {
    match docker_credential::get_credential(registry) {
        Ok(DockerCredential::UsernamePassword(username, password)) => {
            return Ok(Some(RegistryAuth::Basic(username, password)));
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

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_env_var_sanitizes_registry_name() {
        assert_eq!(
            registry_auth_env_var("example.com", "BEARER"),
            "WKG_REGISTRY_EXAMPLE_COM_AUTH_BEARER"
        );
        assert_eq!(
            registry_auth_env_var("ghcr.io", "BEARER"),
            "WKG_REGISTRY_GHCR_IO_AUTH_BEARER"
        );
        assert_eq!(
            registry_auth_env_var("localhost:1234", "BEARER"),
            "WKG_REGISTRY_LOCALHOST_1234_AUTH_BEARER",
        );
    }

    #[test]
    fn auth_env_var_upcases_scheme() {
        assert_eq!(
            registry_auth_env_var("example.com", "bearer"),
            "WKG_REGISTRY_EXAMPLE_COM_AUTH_BEARER"
        );
    }
}
