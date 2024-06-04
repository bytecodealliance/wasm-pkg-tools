use async_trait::async_trait;
use bytes::Bytes;
use docker_credential::{CredentialRetrievalError, DockerCredential};
use futures_util::{stream::BoxStream, StreamExt, TryStreamExt};
use oci_distribution::{
    client::ClientConfig, manifest::OciDescriptor, secrets::RegistryAuth, Reference,
};
use secrecy::ExposeSecret;
use semver::Version;

use crate::{
    config::BasicCredentials,
    meta::RegistryMeta,
    source::{PackageSource, VersionInfo},
    Error, PackageRef, Release,
};

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

pub struct OciSource {
    client: oci_wasm::WasmClient,
    oci_registry: String,
    namespace_prefix: Option<String>,
    credentials: Option<BasicCredentials>,
    registry_auth: Option<RegistryAuth>,
}

impl OciSource {
    pub fn new(
        registry: String,
        config: OciConfig,
        registry_meta: RegistryMeta,
    ) -> Result<Self, Error> {
        let OciConfig {
            client_config,
            credentials,
        } = config;
        let client = oci_distribution::Client::new(client_config);
        let client = oci_wasm::WasmClient::new(client);

        let oci_registry = registry_meta.oci_registry.unwrap_or(registry);

        Ok(Self {
            client,
            oci_registry,
            namespace_prefix: registry_meta.oci_namespace_prefix,
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
                        return Err(err.into());
                    }
                }
                Err(err) => return Err(err.into()),
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

        tracing::debug!("Listing tags for OCI reference {reference:?}");
        let auth = self.auth(&reference).await?;
        let resp = self.client.list_tags(&reference, &auth, None, None).await?;
        tracing::trace!("List tags response: {resp:?}");

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
                    tracing::warn!("Ignoring invalid version tag {tag:?}: {err:?}");
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
            .await?;
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
            .await?;
        Ok(stream.map_err(Into::into).boxed())
    }
}
