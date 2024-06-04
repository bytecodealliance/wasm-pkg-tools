use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{stream::BoxStream, StreamExt, TryStreamExt};
use oci_distribution::{manifest::OciDescriptor, Reference, RegistryOperation};
use semver::Version;
use wasm_pkg_common::oci::OciConfig;

use crate::{
    meta::RegistryMeta,
    source::{PackageSource, VersionInfo},
    Error, PackageRef, Release,
};

pub struct OciSource {
    client: wasm_pkg_common::oci::Oci,
    oci_registry: String,
    namespace_prefix: Option<String>,
}

impl OciSource {
    pub fn new(registry: String, config: OciConfig, registry_meta: RegistryMeta) -> Self {
        let oci_registry = registry_meta.oci_registry.unwrap_or(registry);

        Self {
            client: wasm_pkg_common::oci::Oci::new(config),
            namespace_prefix: registry_meta.oci_namespace_prefix,
            oci_registry,
        }
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
        // NOTE: This error mapping is kinda cheating until we pull out common error types
        // completely. But we know it can only fail with an auth error
        let auth = self
            .client
            .get_auth(&reference, RegistryOperation::Pull)
            .await
            .map_err(|e| Error::CredentialError(e.into()))?;

        let resp = self.client.list_tags(&reference, &auth, None, None).await?;
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
        let auth = self
            .client
            .get_auth(&reference, RegistryOperation::Pull)
            .await
            .map_err(|e| Error::CredentialError(e.into()))?;
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
        self.client
            .get_auth(&reference, RegistryOperation::Pull)
            .await
            .map_err(|e| Error::CredentialError(e.into()))?;
        let stream = self
            .client
            .pull_blob_stream(&reference, &descriptor)
            .await?;
        Ok(stream.map_err(Into::into).boxed())
    }
}
