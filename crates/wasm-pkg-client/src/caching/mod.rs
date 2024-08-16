use std::future::Future;

use wasm_pkg_common::{
    digest::ContentDigest,
    package::{PackageRef, Version},
    Error,
};

use crate::{Client, ContentStream, Release, VersionInfo};

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

    /// Puts the release data into the cache.
    fn put_release(
        &self,
        package: &PackageRef,
        release: &Release,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    /// Gets the release data from the cache. Returns None if the data is not in the cache.
    fn get_release(
        &self,
        package: &PackageRef,
        version: &Version,
    ) -> impl Future<Output = Result<Option<Release>, Error>> + Send;
}

/// A client that caches response data using the given cache implementation. Can be used without an
/// underlying client to be used as a read-only cache.
pub struct CachingClient<T> {
    client: Option<Client>,
    cache: T,
}

impl<T: Cache> CachingClient<T> {
    /// Creates a new caching client from the given client and cache implementation. If no client is
    /// given, the client will be in offline or read-only mode, meaning it will only be able to return
    /// things that are already in the cache.
    pub fn new(client: Option<Client>, cache: T) -> Self {
        Self { client, cache }
    }

    /// Returns whether or not the client is in read-only mode.
    pub fn is_readonly(&self) -> bool {
        self.client.is_none()
    }

    /// Returns a list of all package [`VersionInfo`]s available for the given package. This will
    /// always fail if no client was provided.
    pub async fn list_all_versions(&self, package: &PackageRef) -> Result<Vec<VersionInfo>, Error> {
        let client = self.client()?;
        client.list_all_versions(package).await
    }

    /// Returns a [`Release`] for the given package version.
    pub async fn get_release(
        &self,
        package: &PackageRef,
        version: &Version,
    ) -> Result<Release, Error> {
        if let Some(data) = self.cache.get_release(package, version).await? {
            return Ok(data);
        }

        let client = self.client()?;
        let release = client.get_release(package, version).await?;
        self.cache.put_release(package, &release).await?;
        Ok(release)
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

        let client = self.client()?;
        let stream = client.stream_content(package, release).await?;
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

    fn client(&self) -> Result<&Client, Error> {
        self.client
            .as_ref()
            .ok_or_else(|| Error::CacheError(anyhow::anyhow!("Client is in read only mode")))
    }
}
