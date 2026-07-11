//! Local filesystem-based package backend.
//!
//! Each package release is a file: `<root-dir>/<namespace>/<name>/<version>.wasm`

use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::anyhow;
use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio_util::io::ReaderStream;
use wasm_pkg_common::{
    Error,
    config::RegistryConfig,
    digest::ContentDigest,
    metadata::LOCAL_PROTOCOL,
    package::{PackageRef, Version},
};

use crate::{
    ContentStream, PublishingSource,
    loader::PackageLoader,
    publisher::PackagePublisher,
    release::{Release, VersionInfo},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LocalConfig {
    pub root: PathBuf,
    // NOTE: set by [`Self::temp_dir`] to avoid holding onto a separate `TempDir` handle.
    #[serde(skip)]
    #[doc(hidden)]
    _temp_handle: Option<Arc<TempDir>>,
}

impl LocalConfig {
    /// Creates a [`Self`] with a new temporary directory.
    /// The returned config owns the directory and removes the config upon drop.
    pub fn temp_dir() -> Result<Self, Error> {
        let handle = TempDir::new()?;
        let root = handle.path().to_path_buf();
        tracing::debug!(registry_dir = %root.display(), "created temporary directory");
        Ok(Self {
            root,
            _temp_handle: Some(Arc::new(handle)),
        })
    }
}

#[derive(Clone)]
pub(crate) struct LocalBackend {
    pub(crate) root: PathBuf,
}

fn registry_path_context(err: io::Error, path: &Path) -> Error {
    let err = anyhow::Error::new(err).context(format!("path: {}", path.display()));
    Error::RegistryError(err)
}

impl LocalBackend {
    pub fn new(registry_config: RegistryConfig) -> Result<Self, Error> {
        let config = registry_config
            .backend_config::<LocalConfig>(LOCAL_PROTOCOL)?
            .ok_or_else(|| {
                Error::InvalidConfig(anyhow!("'local' backend requires configuration"))
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

        let mut entries = match tokio::fs::read_dir(&package_dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::PackageNotFound);
            }
            Err(e) => return Err(registry_path_context(e, &package_dir)),
        };
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
        let content_digest = sha256_from_file(&path)
            .await
            .map_err(|e| registry_path_context(e, &path))?;
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
        let file = tokio::fs::File::open(&path)
            .await
            .map_err(|e| registry_path_context(e, &path))?;
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
        dry_run: bool,
    ) -> Result<(), Error> {
        let package_dir = self.package_dir(package);
        // Ensure the package directory exists.
        tokio::fs::create_dir_all(&package_dir)
            .await
            .map_err(|e| registry_path_context(e, &package_dir))?;
        let path = self.version_path(package, version);
        if dry_run {
            return Ok(());
        }
        let mut out = tokio::fs::File::create(&path)
            .await
            .map_err(|e| registry_path_context(e, &path))?;
        tracing::info!("publishing to {}", path.display());
        tokio::io::copy(&mut data, &mut out)
            .await
            .map_err(Error::IoError)
            .map(|_| ())
    }
}

async fn sha256_from_file(path: impl AsRef<Path>) -> Result<ContentDigest, std::io::Error> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = [0; 4096];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.into())
}
