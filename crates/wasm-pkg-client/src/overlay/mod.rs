use async_trait::async_trait;
use tempfile::TempDir;
use wasm_pkg_common::Error;

use crate::{loader::PackageLoader, local::LocalBackend, publisher::PackagePublisher, InnerClient};

pub(crate) struct OverlayBackend {
    local: LocalBackend,
    remote: InnerClient,
    _handle: TempDir,
}

impl OverlayBackend {
    fn new(remote: InnerClient) -> Result<Self, Error> {
        let handle = TempDir::new()?;
        let root = handle.path().to_owned();
        let local = LocalBackend { root };
        Ok(Self {
            local,
            remote,
            _handle: handle,
        })
    }
}

#[async_trait]
impl PackageLoader for OverlayBackend {
    async fn list_all_versions(&self, package: &PackageRef) -> Result<Vec<VersionInfo>, Error> {
        let mut versions = self.local.list_all_versions(package).await?;
        let mut remote_versions = self.remote.list_all_versions(package).await?;
        versions.append(&mut remote_versions);
        versions.sort();
        versions.dedup();
        Ok(versions)
    }

    async fn get_release(&self, package: &PackageRef, version: &Version) -> Result<Release, Error> {
        if let Ok(release) = self.local.get_release(package, version).await {
            return Ok(release);
        }
        tracing::debug!(%package, %version, method = "get_release", "OverlayBackend falling back to remote");
        self.remote.get_release(package, version).await
    }

    async fn stream_content_unvalidated(
        &self,
        package: &PackageRef,
        content: &Release,
    ) -> Result<ContentStream, Error> {
        if let Ok(stream) = self
            .local
            .stream_content_unvalidated(package, content)
            .await
        {
            return Ok(stream);
        }
        tracing::debug!(%package, %version, method = "stream_content_unvalidated", "OverlayBackend falling back to remote");

        self.local
            .stream_content_unvalidated(package, content)
            .await
    }
}

#[async_trait::async_trait]
impl PackagePublisher for LocalBackend {
    async fn publish(
        &self,
        package: &PackageRef,
        version: &Version,
        mut data: PublishingSource,
        dry_run: bool,
    ) -> Result<(), Error> {
        self.local
            .publish(&package, &version, data, additional_options.dry_run)
            .await?;
        if dry_run {
            return Ok(());
        }
        self.remote
            .publish(&package, &version, data, additional_options.dry_run)
            .await?;

        Ok(())
    }
}
