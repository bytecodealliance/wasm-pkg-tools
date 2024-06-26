//! A `Cache` implementation for a filesystem

use std::path::{Path, PathBuf};

use anyhow::Context;
use futures_util::{StreamExt, TryStreamExt};
use tokio_util::io::{ReaderStream, StreamReader};
use wasm_pkg_common::{digest::ContentDigest, Error};

use crate::loader::ContentStream;

use super::Cache;

pub struct FileCache {
    root: PathBuf,
}

impl FileCache {
    pub async fn new(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(&root)
            .await
            .context("Unable to create cache directory")?;
        Ok(Self {
            root: root.as_ref().to_path_buf(),
        })
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
}
