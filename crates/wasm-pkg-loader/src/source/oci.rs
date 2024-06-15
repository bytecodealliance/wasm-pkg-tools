mod config;

use async_trait::async_trait;
use bytes::Bytes;
use config::{BasicCredentials, OciConfig};
use docker_credential::{CredentialRetrievalError, DockerCredential};
use futures_util::{stream::BoxStream, StreamExt, TryStreamExt};
use oci_distribution::{
    errors::OciDistributionError, manifest::OciDescriptor, secrets::RegistryAuth, Reference,
};
use secrecy::ExposeSecret;
use serde::Deserialize;
use wasm_pkg_common::{
    config::RegistryConfig,
    metadata::RegistryMetadata,
    package::{PackageRef, Version},
    registry::Registry,
    Error,
};

use crate::{
    source::{PackageSource, VersionInfo},
    Release,
};

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OciRegistryMetadata {
    registry: Option<String>,
    namespace_prefix: Option<String>,
}

pub struct OciSource {
    client: oci_wasm::WasmClient,
    oci_registry: String,
    namespace_prefix: Option<String>,
    credentials: Option<BasicCredentials>,
    registry_auth: Option<RegistryAuth>,
}

impl OciSource {
    pub fn new(
        registry: &Registry,
        registry_config: &RegistryConfig,
        registry_meta: &RegistryMetadata,
    ) -> Result<Self, Error> {
        let OciConfig {
            client_config,
            credentials,
        } = registry_config.try_into()?;
        let client = oci_distribution::Client::new(client_config);
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
            registry_auth: None,
        })
    }

    async fn auth(&mut self, reference: &Reference) -> Result<RegistryAuth, Error> {
        if self.registry_auth.is_none() {
            let mut auth = self.get_credentials()?;
            // Preflight auth to check for validity; this isn't wasted
            // effort because the oci_distribution::Client caches it
            use oci_distribution::errors::OciDistributionError::AuthenticationFailure;
            use oci_distribution::RegistryOperation::Pull;
            match self.client.auth(reference, &auth, Pull).await {
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
                        return Err(oci_registry_error(err));
                    }
                }
                Err(err) => return Err(oci_registry_error(err)),
            }
            self.registry_auth = Some(auth);
        }
        Ok(self.registry_auth.clone().unwrap())
    }

    fn get_credentials(&self) -> Result<RegistryAuth, Error> {
        if let Some(BasicCredentials { username, password }) = &self.credentials {
            return Ok(RegistryAuth::Basic(
                username.clone(),
                password.expose_secret().clone(),
            ));
        }

        let server_url = format!("https://{}", self.oci_registry);
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
                    tracing::debug!("Failed to look up OCI credentials: {err}");
                } else {
                    tracing::warn!("Failed to look up OCI credentials: {err}");
                };
            }
        }

        Ok(RegistryAuth::Anonymous)
    }

    fn make_reference(&self, package: &PackageRef, version: Option<&Version>) -> Reference {
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

#[async_trait]
impl PackageSource for OciSource {
    async fn list_all_versions(&mut self, package: &PackageRef) -> Result<Vec<VersionInfo>, Error> {
        let reference = self.make_reference(package, None);

        tracing::debug!(?reference, "Listing tags for OCI reference");
        let auth = self.auth(&reference).await?;
        let resp = self
            .client
            .list_tags(&reference, &auth, None, None)
            .await
            .map_err(oci_registry_error)?;
        tracing::trace!(response = ?resp, "List tags response");

        // Return only tags that parse as valid semver versions.
        let versions = resp
            .tags
            .iter()
            .flat_map(|tag| match Version::parse(tag) {
                Ok(version) => Some(VersionInfo {
                    version,
                    yanked: false,
                }),
                Err(err) => {
                    tracing::warn!(?tag, error = ?err, "Ignoring invalid version tag");
                    None
                }
            })
            .collect();
        Ok(versions)
    }

    async fn get_release(
        &mut self,
        package: &PackageRef,
        version: &Version,
    ) -> Result<Release, Error> {
        let reference = self.make_reference(package, Some(version));

        tracing::debug!(?reference, "Fetching image manifest for OCI reference");
        let auth = self.auth(&reference).await?;
        let (manifest, _config, _digest) = self
            .client
            .pull_manifest_and_config(&reference, &auth)
            .await
            .map_err(Error::RegistryError)?;
        tracing::trace!(?manifest, "Got manifest");

        let version = version.to_owned();
        let content_digest = manifest
            .layers
            .into_iter()
            .next()
            .ok_or_else(|| {
                Error::InvalidPackageManifest("Returned manifest had no layers".to_string())
            })?
            .digest
            .parse()?;
        Ok(Release {
            version,
            content_digest,
        })
    }

    async fn stream_content_unvalidated(
        &mut self,
        package: &PackageRef,
        release: &Release,
    ) -> Result<BoxStream<Result<Bytes, Error>>, Error> {
        let reference = self.make_reference(package, None);
        let descriptor = OciDescriptor {
            digest: release.content_digest.to_string(),
            ..Default::default()
        };
        self.auth(&reference).await?;
        let stream = self
            .client
            .pull_blob_stream(&reference, &descriptor)
            .await
            .map_err(oci_registry_error)?;
        Ok(stream.map_err(Into::into).boxed())
    }
}

fn oci_registry_error(err: OciDistributionError) -> Error {
    Error::RegistryError(err.into())
}
