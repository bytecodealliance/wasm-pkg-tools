use crate::publisher::PackagePublisher;
use crate::{PackageRef, Version};

use super::WargBackend;

#[async_trait::async_trait]
impl PackagePublisher for WargBackend {
    async fn publish(
        &self,
        package: &PackageRef,
        version: &Version,
        data: Vec<u8>,
    ) -> Result<(), crate::Error> {
        todo!()
    }
}
