use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{stream::BoxStream, StreamExt};
use wasm_pkg_common::{
    package::{PackageRef, Version},
    Error,
};

use crate::release::{Release, VersionInfo};

#[async_trait]
pub trait PackageLoader: Send {
    async fn list_all_versions(&mut self, package: &PackageRef) -> Result<Vec<VersionInfo>, Error>;

    async fn get_release(
        &mut self,
        package: &PackageRef,
        version: &Version,
    ) -> Result<Release, Error>;

    async fn stream_content_unvalidated(
        &mut self,
        package: &PackageRef,
        release: &Release,
    ) -> Result<BoxStream<Result<Bytes, Error>>, Error>;

    async fn stream_content(
        &mut self,
        package: &PackageRef,
        release: &Release,
    ) -> Result<BoxStream<Result<Bytes, Error>>, Error> {
        let stream = self.stream_content_unvalidated(package, release).await?;
        Ok(release.content_digest.validating_stream(stream).boxed())
    }
}
