use oci_client::{Reference, RegistryOperation};

use crate::publisher::PackagePublisher;
use crate::{PackageRef, Version};

use super::OciBackend;

#[async_trait::async_trait]
impl PackagePublisher for OciBackend {
    async fn publish(
        &self,
        package: &PackageRef,
        version: &Version,
        data: Vec<u8>,
    ) -> Result<(), crate::Error> {
        let (config, layer) = oci_wasm::WasmConfig::from_raw_component(data, None)
            .map_err(crate::Error::InvalidComponent)?;

        let reference: Reference = self.make_reference(package, Some(version));
        let auth = self.auth(&reference, RegistryOperation::Push).await?;
        self.client
            .push(&reference, &auth, layer, config, None)
            .await
            .map_err(crate::Error::RegistryError)?;
        Ok(())
    }
}
