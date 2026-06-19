use std::collections::HashMap;

use async_trait::async_trait;
use tempfile::TempDir;
use wasm_pkg_common::{
    package::{PackageRef, Version},
    Error,
};

use crate::{
    loader::PackageLoader, local::LocalBackend, publisher::PackagePublisher, ContentStream,
    InnerClient, PublishingSource, Release, VersionInfo,
};

pub(crate) struct OverlayBackend {
    local: LocalBackend,
    remotes: HashMap<PackageRef, InnerClient>,
    _handle: TempDir,
}

impl OverlayBackend {
    fn new(remotes: HashMap<PackageRef, InnerClient>) -> Result<Self, Error> {
        let (local, handle) = LocalBackend::temp_dir()?;
        Ok(Self {
            local,
            remotes,
            _handle: handle,
        })
    }

    fn remote(&self, package: &PackageRef) -> Result<&InnerClient, Error> {
        self.remotes
            .get(package)
            .ok_or_else(|| Error::InvalidPackageRef(package.to_string()))
    }
}

#[async_trait]
impl PackageLoader for OverlayBackend {
    async fn list_all_versions(&self, package: &PackageRef) -> Result<Vec<VersionInfo>, Error> {
        let mut versions = self.local.list_all_versions(package).await?;

        if let Some(remote) = self.remotes.get(package) {
            let mut remote_versions = remote.list_all_versions(package).await?;
            versions.append(&mut remote_versions);
            versions.sort();
            versions.dedup();
        }

        Ok(versions)
    }

    async fn get_release(&self, package: &PackageRef, version: &Version) -> Result<Release, Error> {
        if let Ok(release) = self.local.get_release(package, version).await {
            return Ok(release);
        }
        tracing::debug!(%package, %version, method = "get_release", "OverlayBackend falling back to remote");
        self.remote(package)?.get_release(package, version).await
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
        tracing::debug!(%package, version = %content.version, method = "stream_content_unvalidated", "OverlayBackend falling back to remote");

        self.local
            .stream_content_unvalidated(package, content)
            .await
    }
}

#[async_trait::async_trait]
impl PackagePublisher for OverlayBackend {
    async fn publish(
        &self,
        package: &PackageRef,
        version: &Version,
        data: PublishingSource,
        dry_run: bool,
    ) -> Result<(), Error> {
        if dry_run {
            self.local.publish(&package, &version, data, dry_run).await
        } else {
            self.remote(package)?
                .publish(&package, &version, data, dry_run)
                .await
        }
    }
}
