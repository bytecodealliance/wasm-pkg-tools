use crate::{PackageRef, Version};

#[async_trait::async_trait]
pub trait PackagePublisher: Send + Sync {
    /// Publishes the data to the registry. The given data should be a valid wasm component.
    // NOTE(thomastaylor312): We should probably have this take something that is Read + Seek
    // because then we don't have to load into memory. I actually started with this but then
    // realized that I never added a push_blob_stream method to the underlying oci client. We can
    // come back and improve this later as needed. Luckily wasm components are pretty small in the
    // majority of cases.
    async fn publish(
        &self,
        package: &PackageRef,
        version: &Version,
        data: Vec<u8>,
    ) -> Result<(), crate::Error>;
}
