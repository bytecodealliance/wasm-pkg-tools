use async_trait::async_trait;
use futures_util::StreamExt;
use wasm_pkg_common::{
    package::{PackageRef, Version},
    Error,
};

use crate::{
    release::{Release, VersionInfo},
    ContentStream,
};

#[async_trait]
pub trait PackageLoader: Send {
    async fn list_all_versions(&self, package: &PackageRef) -> Result<Vec<VersionInfo>, Error>;

    async fn get_release(&self, package: &PackageRef, version: &Version) -> Result<Release, Error>;

    async fn stream_content_unvalidated(
        &self,
        package: &PackageRef,
        release: &Release,
    ) -> Result<ContentStream, Error>;

    async fn stream_content(
        &self,
        package: &PackageRef,
        release: &Release,
    ) -> Result<ContentStream, Error> {
        let stream = self.stream_content_unvalidated(package, release).await?;
        Ok(release.content_digest.validating_stream(stream).boxed())
    }
}
