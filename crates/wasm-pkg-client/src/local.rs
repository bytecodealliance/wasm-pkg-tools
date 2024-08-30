//! Local filesystem-based package backend.
//!
//! Each package release is a file: `<root>/<namespace>/<name>/<version>.wasm`

use std::path::PathBuf;

use anyhow::anyhow;
use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt};
use serde::Deserialize;
use tokio_util::io::ReaderStream;
use wasm_pkg_common::{
    config::RegistryConfig,
    digest::ContentDigest,
    package::{PackageRef, Version},
    Error,
};

use crate::{
    loader::PackageLoader,
    publisher::PackagePublisher,
    release::{Release, VersionInfo},
    ContentStream, PublishingSource,
};

#[derive(Clone, Debug, Deserialize)]
pub struct LocalConfig {
    pub root: PathBuf,
}

pub(crate) struct LocalBackend {
    root: PathBuf,
}

impl LocalBackend {
    pub fn new(registry_config: RegistryConfig) -> Result<Self, Error> {
        let config = registry_config
            .backend_config::<LocalConfig>("local")?
            .ok_or_else(|| {
                Error::InvalidConfig(anyhow!("'local' backend require configuration"))
            })?;
        Ok(Self { root: config.root })
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
impl PackageLoader for LocalBackend {
    async fn list_all_versions(&self, package: &PackageRef) -> Result<Vec<VersionInfo>, Error> {
        let mut versions = vec![];
        let package_dir = self.package_dir(package);
        tracing::debug!(?package_dir, "Reading versions from path");
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
            versions.push(VersionInfo {
                version,
                yanked: false,
            });
        }
        Ok(versions)
    }

    async fn get_release(&self, package: &PackageRef, version: &Version) -> Result<Release, Error> {
        let path = self.version_path(package, version);
        tracing::debug!(path = %path.display(), "Reading content from path");
        let content_digest = ContentDigest::sha256_from_file(path).await?;
        Ok(Release {
            version: version.clone(),
            content_digest,
        })
    }

    async fn stream_content_unvalidated(
        &self,
        package: &PackageRef,
        content: &Release,
    ) -> Result<ContentStream, Error> {
        let path = self.version_path(package, &content.version);
        tracing::debug!("Streaming content from {path:?}");
        let file = tokio::fs::File::open(path).await?;
        Ok(ReaderStream::new(file).map_err(Into::into).boxed())
    }
}

#[async_trait::async_trait]
impl PackagePublisher for LocalBackend {
    async fn publish(
        &self,
        package: &PackageRef,
        version: &Version,
        mut data: PublishingSource,
    ) -> Result<(), Error> {
        let package_dir = self.package_dir(package);
        // Ensure the package directory exists.
        tokio::fs::create_dir_all(package_dir).await?;
        let path = self.version_path(package, version);
        let mut out = tokio::fs::File::create(path).await?;
        tokio::io::copy(&mut data, &mut out)
            .await
            .map_err(Error::IoError)
            .map(|_| ())
    }
}
