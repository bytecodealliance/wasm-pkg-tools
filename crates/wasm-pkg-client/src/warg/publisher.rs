use std::time::Duration;

use futures_util::TryStreamExt;
use tokio_util::codec::{BytesCodec, FramedRead};
use warg_client::storage::{ContentStorage, PublishEntry, PublishInfo};

use crate::publisher::PackagePublisher;
use crate::{PackageRef, PublishingSource, Version};

use super::WargBackend;

const DEFAULT_WAIT_INTERVAL: Duration = Duration::from_secs(1);

#[async_trait::async_trait]
impl PackagePublisher for WargBackend {
    async fn publish(
        &self,
        package: &PackageRef,
        version: &Version,
        data: PublishingSource,
    ) -> Result<(), crate::Error> {
        // store the Wasm in Warg cache, so that it is available to Warg client for uploading
        let content = self
            .client
            .content()
            .store_content(
                Box::pin(
                    FramedRead::new(data, BytesCodec::new())
                        .map_ok(|b| b.freeze())
                        .map_err(anyhow::Error::from),
                ),
                None,
            )
            .await
            .map_err(crate::Error::RegistryError)?;

        // convert package name to Warg package name
        let name = super::package_ref_to_name(package)?;

        // start Warg publish, using the keyring to sign
        let version = version.clone();
        let info = PublishInfo {
            name: name.clone(),
            head: None,
            entries: vec![PublishEntry::Release { version, content }],
        };
        let record_id = if let Some(key) = self.signing_key.as_ref() {
            self.client.publish_with_info(key, info).await
        } else {
            self.client.sign_with_keyring_and_publish(Some(info)).await
        }
        .map_err(super::warg_registry_error)?;

        // wait for the Warg publish to finish
        self.client
            .wait_for_publish(&name, &record_id, DEFAULT_WAIT_INTERVAL)
            .await
            .map_err(super::warg_registry_error)?;

        Ok(())
    }
}
