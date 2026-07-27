use std::collections::BTreeMap;

use oci_client::{
    Reference, RegistryOperation,
    annotations::{
        ORG_OPENCONTAINERS_IMAGE_AUTHORS, ORG_OPENCONTAINERS_IMAGE_DESCRIPTION,
        ORG_OPENCONTAINERS_IMAGE_LICENSES, ORG_OPENCONTAINERS_IMAGE_SOURCE,
        ORG_OPENCONTAINERS_IMAGE_TITLE, ORG_OPENCONTAINERS_IMAGE_URL,
        ORG_OPENCONTAINERS_IMAGE_VERSION,
    },
};
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
        dry_run: bool,
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
        let (config, mut layer) = oci_wasm::WasmConfig::from_raw_component(buf, None)
            .map_err(crate::Error::InvalidComponent)?;
        // Set the layer title so OCI tools can name the file on disk
        layer.annotations = Some(BTreeMap::from_iter([(
            ORG_OPENCONTAINERS_IMAGE_TITLE.to_string(),
            format!("{}.wasm", package.name()),
        )]));
        let mut annotations = BTreeMap::from_iter([(
            ORG_OPENCONTAINERS_IMAGE_VERSION.to_string(),
            version.to_string(),
        )]);
        if let Some(desc) = &meta.description {
            annotations.insert(
                ORG_OPENCONTAINERS_IMAGE_DESCRIPTION.to_string(),
                desc.to_string(),
            );
        }
        if let Some(licenses) = &meta.licenses {
            annotations.insert(
                ORG_OPENCONTAINERS_IMAGE_LICENSES.to_string(),
                licenses.to_string(),
            );
        }
        if let Some(source) = &meta.source {
            annotations.insert(
                ORG_OPENCONTAINERS_IMAGE_SOURCE.to_string(),
                source.to_string(),
            );
        }
        if let Some(homepage) = &meta.homepage {
            annotations.insert(
                ORG_OPENCONTAINERS_IMAGE_URL.to_string(),
                homepage.to_string(),
            );
        }
        if let Some(authors) = &meta.authors {
            annotations.insert(
                ORG_OPENCONTAINERS_IMAGE_AUTHORS.to_string(),
                authors.to_string(),
            );
        }

        let reference: Reference = self.make_reference(package, Some(version));
        let auth = self.auth(&reference, RegistryOperation::Push).await?;
        if !dry_run {
            self.client
                .push(&reference, &auth, layer, config, Some(annotations))
                .await
                .map_err(crate::Error::RegistryError)?;
        }
        Ok(())
    }
}
