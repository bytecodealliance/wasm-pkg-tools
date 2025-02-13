use std::collections::BTreeMap;

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
        let payload = wasm_metadata::Payload::from_binary(&buf).map_err(|e| {
            crate::Error::InvalidComponent(anyhow::anyhow!("Unable to parse WASM: {e}"))
        })?;
        let meta = payload.metadata();
        let (config, layer) = oci_wasm::WasmConfig::from_raw_component(buf, None)
            .map_err(crate::Error::InvalidComponent)?;
        let mut annotations = BTreeMap::from_iter([(
            "org.opencontainers.image.version".to_string(),
            version.to_string(),
        )]);
        if let Some(desc) = &meta.description {
            annotations.insert(
                "org.opencontainers.image.description".to_string(),
                desc.to_string(),
            );
        }
        if let Some(licenses) = &meta.licenses {
            annotations.insert(
                "org.opencontainers.image.licenses".to_string(),
                licenses.to_string(),
            );
        }
        if let Some(source) = &meta.source {
            annotations.insert(
                "org.opencontainers.image.source".to_string(),
                source.to_string(),
            );
        }
        if let Some(homepage) = &meta.homepage {
            annotations.insert(
                "org.opencontainers.image.url".to_string(),
                homepage.to_string(),
            );
        }
        if let Some(authors) = &meta.author {
            annotations.insert(
                "org.opencontainers.image.authors".to_string(),
                authors.to_string(),
            );
        }

        let reference: Reference = self.make_reference(package, Some(version));
        let auth = self.auth(&reference, RegistryOperation::Push).await?;
        self.client
            .push(&reference, &auth, layer, config, Some(annotations))
            .await
            .map_err(crate::Error::RegistryError)?;
        Ok(())
    }
}
