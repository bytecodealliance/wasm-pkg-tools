use oci_client::{Reference, RegistryOperation};
use tokio::io::AsyncReadExt;

use crate::publisher::PackagePublisher;
use crate::{PackageRef, PublishingSource, Version};

use super::OciBackend;

#[async_trait::async_trait]
impl PackagePublisher for OciBackend {
    async fn publish(
        &self,
        package: &PackageRef,
        version: &Version,
        mut data: PublishingSource,
    ) -> Result<(), crate::Error> {
        // NOTE(thomastaylor312): oci-client doesn't support publishing from a stream or reader, so
        // we have to read all the data in for now. Once we can address that upstream, we'll be able
        // to remove this and use the stream directly.
        let mut buf = Vec::new();
        data.read_to_end(&mut buf).await?;
        let (config, layer) = oci_wasm::WasmConfig::from_raw_component(buf, None)
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
