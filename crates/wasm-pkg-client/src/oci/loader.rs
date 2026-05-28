use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt};
use oci_client::{manifest::OciDescriptor, RegistryOperation};
use wasm_pkg_common::{
    package::{PackageRef, Version},
    Error,
};

use crate::{
    loader::PackageLoader,
    release::{Release, VersionInfo},
    ContentStream,
};

use super::{oci_registry_error, OciBackend};

#[async_trait]
impl PackageLoader for OciBackend {
    async fn list_all_versions(&self, package: &PackageRef) -> Result<Vec<VersionInfo>, Error> {
        let reference = self.make_reference(package, None);

        tracing::debug!(?reference, "Listing tags for OCI reference");
        let auth = self.auth(&reference, RegistryOperation::Pull).await?;
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
                    // Signature tags all start with a SHA and shouldn't generate a warning
                    if tag.starts_with("sha256-") {
                        tracing::debug!(?tag, "Ignoring signature tag");
                    } else {
                        tracing::warn!(?tag, error = ?err, "Ignoring invalid version tag");
                    }
                    None
                }
            })
            .collect();
        Ok(versions)
    }

    async fn get_release(&self, package: &PackageRef, version: &Version) -> Result<Release, Error> {
        let reference = self.make_reference(package, Some(version));

        tracing::debug!(?reference, "Fetching image manifest for OCI reference");
        let auth = self.auth(&reference, RegistryOperation::Pull).await?;
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
        &self,
        package: &PackageRef,
        release: &Release,
    ) -> Result<ContentStream, Error> {
        let reference = self.make_reference(package, None);
        let descriptor = OciDescriptor {
            digest: release.content_digest.to_string(),
            ..Default::default()
        };
        self.auth(&reference, RegistryOperation::Pull).await?;
        let stream = self
            .client
            .pull_blob_stream(&reference, &descriptor)
            .await
            .map_err(oci_registry_error)?;
        Ok(stream.map_err(Into::into).boxed())
    }
}
