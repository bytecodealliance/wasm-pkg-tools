//! A `Cache` implementation for a filesystem

use std::path::{Path, PathBuf};

use anyhow::Context;
use etcetera::BaseStrategy;
use futures_util::{StreamExt, TryStreamExt};
use tokio_util::io::{ReaderStream, StreamReader};
use wasm_pkg_common::{
    digest::ContentDigest,
    package::{PackageRef, Version},
    Error,
};

use crate::{ContentStream, Release};

use super::Cache;

pub struct FileCache {
    root: PathBuf,
}

impl FileCache {
    /// Creates a new file cache that stores data in the given directory.
    pub async fn new(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(&root)
            .await
            .context("Unable to create cache directory")?;
        Ok(Self {
            root: root.as_ref().to_path_buf(),
        })
    }

    /// Returns a cache setup to use the global default cache path if it can be determined,
    /// otherwise this will error
    pub async fn global_cache() -> anyhow::Result<Self> {
        Self::new(Self::global_cache_path().context("couldn't find global cache path")?).await
    }

    /// Returns the global default cache path if it can be determined, otherwise returns None
    pub fn global_cache_path() -> Option<PathBuf> {
        etcetera::choose_base_strategy()
            .ok()
            .map(|strat| strat.cache_dir().join("wasm-pkg"))
    }
}

#[derive(serde::Serialize)]
struct ReleaseInfoBorrowed<'a> {
    version: &'a Version,
    content_digest: &'a ContentDigest,
}

impl<'a> From<&'a Release> for ReleaseInfoBorrowed<'a> {
    fn from(release: &'a Release) -> Self {
        Self {
            version: &release.version,
            content_digest: &release.content_digest,
        }
    }
}

#[derive(serde::Deserialize)]
struct ReleaseInfoOwned {
    version: Version,
    content_digest: ContentDigest,
}

impl From<ReleaseInfoOwned> for Release {
    fn from(info: ReleaseInfoOwned) -> Self {
        Self {
            version: info.version,
            content_digest: info.content_digest,
        }
    }
}

impl Cache for FileCache {
    async fn put_data(&self, digest: ContentDigest, data: ContentStream) -> Result<(), Error> {
        let path = self.root.join(digest.to_string());
        let mut file = tokio::fs::File::create(&path).await.map_err(|e| {
            Error::CacheError(anyhow::anyhow!("Unable to create file for cache {e}"))
        })?;
        let mut buf =
            StreamReader::new(data.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)));
        tokio::io::copy(&mut buf, &mut file)
            .await
            .map_err(|e| Error::CacheError(e.into()))
            .map(|_| ())
    }

    async fn get_data(&self, digest: &ContentDigest) -> Result<Option<ContentStream>, Error> {
        let path = self.root.join(digest.to_string());
        let exists = tokio::fs::try_exists(&path)
            .await
            .map_err(|e| Error::CacheError(e.into()))?;
        if !exists {
            return Ok(None);
        }
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|e| Error::CacheError(e.into()))?;

        Ok(Some(
            ReaderStream::new(file).map_err(Error::IoError).boxed(),
        ))
    }

    async fn put_release(&self, package: &PackageRef, release: &Release) -> Result<(), Error> {
        let path = self
            .root
            .join(format!("{}-{}.json", package, release.version));
        tokio::fs::write(
            path,
            serde_json::to_string(&ReleaseInfoBorrowed::from(release)).map_err(|e| {
                Error::CacheError(anyhow::anyhow!("Error serializing data to disk: {e}"))
            })?,
        )
        .await
        .map(|_| ())
        .map_err(|e| Error::CacheError(anyhow::anyhow!("Error writing to disk: {e}")))
    }

    async fn get_release(
        &self,
        package: &PackageRef,
        version: &Version,
    ) -> Result<Option<Release>, Error> {
        let path = self.root.join(format!("{}-{}.json", package, version));
        let exists = tokio::fs::try_exists(&path).await.map_err(|e| {
            Error::CacheError(anyhow::anyhow!("Error checking if file exists: {e}"))
        })?;
        if !exists {
            return Ok(None);
        }
        let data = tokio::fs::read(path)
            .await
            .map_err(|e| Error::CacheError(anyhow::anyhow!("Error reading from disk: {e}")))?;
        let release: ReleaseInfoOwned = serde_json::from_slice(&data).map_err(|e| {
            Error::CacheError(anyhow::anyhow!("Error deserializing data from disk: {e}"))
        })?;
        Ok(Some(release.into()))
    }
}
