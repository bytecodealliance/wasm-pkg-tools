use anyhow::anyhow;
use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt};
use wasm_pkg_common::{
    package::{PackageRef, Version},
    Error,
};

use crate::{
    loader::{ContentStream, PackageLoader},
    release::{Release, VersionInfo},
};

use super::{package_ref_to_name, warg_registry_error, WargBackend};

#[async_trait]
impl PackageLoader for WargBackend {
    async fn list_all_versions(&self, package: &PackageRef) -> Result<Vec<VersionInfo>, Error> {
        let info = self.fetch_package_info(package).await?;
        Ok(info
            .state
            .releases()
            .map(|r| VersionInfo {
                version: r.version.clone(),
                yanked: r.yanked(),
            })
            .collect())
    }

    async fn get_release(&self, package: &PackageRef, version: &Version) -> Result<Release, Error> {
        let info = self.fetch_package_info(package).await?;
        let release = info
            .state
            .release(version)
            .ok_or_else(|| Error::VersionNotFound(version.clone()))?;
        let content_digest = release
            .content()
            .ok_or_else(|| Error::RegistryError(anyhow!("version {version} yanked")))?
            .to_string();
        Ok(Release {
            version: version.clone(),
            content_digest: content_digest.parse()?,
        })
    }

    async fn stream_content_unvalidated(
        &self,
        package: &PackageRef,
        release: &Release,
    ) -> Result<ContentStream, Error> {
        self.stream_content(package, release).await
    }

    async fn stream_content(
        &self,
        package: &PackageRef,
        release: &Release,
    ) -> Result<ContentStream, Error> {
        let package_name = package_ref_to_name(package)?;

        // warg client validates the digest matches the content
        let (_, stream) = self
            .client
            .download_exact_as_stream(&package_name, &release.version)
            .await
            .map_err(warg_registry_error)?;
        Ok(stream.map_err(Error::RegistryError).boxed())
    }
}
