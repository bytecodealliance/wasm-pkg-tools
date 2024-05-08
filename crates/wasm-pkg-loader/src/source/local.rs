use std::path::PathBuf;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{stream::BoxStream, StreamExt, TryStreamExt};
use semver::Version;
use tokio_util::io::ReaderStream;

use crate::{source::PackageSource, ContentDigest, Error, PackageRef, Release};

#[derive(Clone, Debug)]
pub struct LocalConfig {
    pub root: PathBuf,
}

/// A simple local filesystem-based PackageSource.
///
/// Each package release is a file: `<root>/<namespace>/<name>/<version>.wasm`
pub struct LocalSource {
    root: PathBuf,
}

impl LocalSource {
    pub fn new(config: LocalConfig) -> Self {
        Self { root: config.root }
    }

    fn package_dir(&self, package: &PackageRef) -> PathBuf {
        self.root
            .join(package.namespace().as_ref())
            .join(package.name().as_ref())
    }

    fn version_path(&self, package: &PackageRef, version: &Version) -> PathBuf {
        self.package_dir(package).join(format!("{version}.wasm"))
    }
}

#[async_trait]
impl PackageSource for LocalSource {
    async fn list_all_versions(&mut self, package: &PackageRef) -> Result<Vec<Version>, Error> {
        let mut versions = vec![];
        let package_dir = self.package_dir(package);
        tracing::debug!("Reading versions from {package_dir:?}");
        let mut entries = tokio::fs::read_dir(package_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension() != Some("wasm".as_ref()) {
                continue;
            }
            let Some(version) = path
                .file_stem()
                .unwrap()
                .to_str()
                .and_then(|stem| Version::parse(stem).ok())
            else {
                tracing::warn!("invalid package file name at {path:?}");
                continue;
            };
            versions.push(version);
        }
        Ok(versions)
    }

    async fn get_release(
        &mut self,
        package: &PackageRef,
        version: &Version,
    ) -> Result<Release, Error> {
        let path = self.version_path(package, version);
        tracing::debug!("Reading content from {path:?}");
        let content_digest = ContentDigest::sha256_from_file(path).await?;
        Ok(Release {
            version: version.clone(),
            content_digest,
        })
    }

    async fn stream_content_unvalidated(
        &mut self,
        package: &PackageRef,
        content: &Release,
    ) -> Result<BoxStream<Result<Bytes, Error>>, Error> {
        let path = self.version_path(package, &content.version);
        tracing::debug!("Streaming content from {path:?}");
        let file = tokio::fs::File::open(path).await?;
        Ok(ReaderStream::new(file).map_err(Into::into).boxed())
    }
}
