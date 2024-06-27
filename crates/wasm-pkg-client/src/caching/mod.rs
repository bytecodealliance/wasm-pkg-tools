use std::future::Future;

use wasm_pkg_common::{digest::ContentDigest, package::PackageRef, Error};

use crate::{loader::ContentStream, Client, Release};

mod file;

pub use file::FileCache;

/// A trait for a cache of data.
pub trait Cache {
    /// Puts the data with the given hash into the cache
    fn put_data(
        &self,
        digest: ContentDigest,
        data: ContentStream,
    ) -> impl Future<Output = Result<(), Error>> + Send;
    /// Gets the data with the given hash from the cache. Returns None if the data is not in the cache.
    fn get_data(
        &self,
        digest: &ContentDigest,
    ) -> impl Future<Output = Result<Option<ContentStream>, Error>> + Send;
}

pub struct CachingClient<T> {
    /// The underlying client, available for use for its other methods that do not require the cache.
    pub client: Client,
    cache: T,
}

impl<T> AsRef<Client> for CachingClient<T> {
    fn as_ref(&self) -> &Client {
        &self.client
    }
}

impl<T: Cache> CachingClient<T> {
    pub fn new(client: Client, cache: T) -> Self {
        Self { client, cache }
    }

    /// Returns a [`ContentStream`] of content chunks. If the data is in the cache, it will be returned,
    /// otherwise it will be fetched from an upstream registry and then cached. This is the same as
    /// [`Client::stream_content`] but named differently to avoid confusion when trying to use this
    /// as a normal [`Client`].
    pub async fn get_content(
        &self,
        package: &PackageRef,
        release: &Release,
    ) -> Result<ContentStream, Error> {
        if let Some(data) = self.cache.get_data(&release.content_digest).await? {
            return Ok(data);
        }

        let stream = self.client.stream_content(package, release).await?;
        self.cache
            .put_data(release.content_digest.clone(), stream)
            .await?;

        self.cache
            .get_data(&release.content_digest)
            .await?
            .ok_or_else(|| {
                Error::CacheError(anyhow::anyhow!(
                    "Cached data was deleted after putting the data in cache"
                ))
            })
    }
}
