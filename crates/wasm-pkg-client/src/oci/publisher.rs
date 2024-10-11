use std::collections::BTreeMap;

use oci_client::{Reference, RegistryOperation};
use tokio::io::AsyncReadExt;
use wasm_metadata::LinkType;

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
        let meta = wasm_metadata::RegistryMetadata::from_wasm(&buf).map_err(|e| {
            crate::Error::InvalidComponent(anyhow::anyhow!("Unable to parse component: {e}"))
        })?;
        let (config, layer) = oci_wasm::WasmConfig::from_raw_component(buf, None)
            .map_err(crate::Error::InvalidComponent)?;
        let mut annotations = BTreeMap::from_iter([(
            "org.opencontainers.image.version".to_string(),
            version.to_string(),
        )]);
        if let Some(meta) = meta {
            if let Some(desc) = meta.get_description() {
                annotations.insert(
                    "org.opencontainers.image.description".to_string(),
                    desc.to_owned(),
                );
            }
            if let Some(licenses) = meta.get_license() {
                annotations.insert(
                    "org.opencontainers.image.licenses".to_string(),
                    licenses.to_owned(),
                );
            }
            if let Some(sources) = meta.get_links() {
                for link in sources {
                    if link.ty == LinkType::Repository {
                        annotations.insert(
                            "org.opencontainers.image.source".to_string(),
                            link.value.to_owned(),
                        );
                    }
                    if link.ty == LinkType::Homepage {
                        annotations.insert(
                            "org.opencontainers.image.url".to_string(),
                            link.value.to_owned(),
                        );
                    }
                }
            }
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
