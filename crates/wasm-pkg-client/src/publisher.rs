use crate::{PackageRef, PublishingSource, Version};

#[async_trait::async_trait]
pub trait PackagePublisher: Send + Sync {
    /// Publishes the data to the registry. The given data should be a valid wasm component and can
    /// be anything that implements [`AsyncRead`](tokio::io::AsyncRead) and
    /// [`AsyncSeek`](tokio::io::AsyncSeek).
    async fn publish(
        &self,
        package: &PackageRef,
        version: &Version,
        data: PublishingSource,
    ) -> Result<(), crate::Error>;
}
