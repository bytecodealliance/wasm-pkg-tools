use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{stream::BoxStream, StreamExt};
use semver::Version;

use crate::{Error, PackageRef, Release};

pub mod local;
pub mod oci;
pub mod warg;

#[async_trait]
pub trait PackageSource: Send {
    async fn list_all_versions(&mut self, package: &PackageRef) -> Result<Vec<Version>, Error>;

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
