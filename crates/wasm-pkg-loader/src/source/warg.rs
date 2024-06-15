mod config;

use anyhow::anyhow;
use async_trait::async_trait;
use bytes::Bytes;
use config::WargConfig;
use futures_util::{stream::BoxStream, StreamExt, TryStreamExt};
use serde::Deserialize;
use warg_client::{storage::PackageInfo, ClientError, FileSystemClient};
use warg_protocol::registry::PackageName;
use wasm_pkg_common::{
    config::RegistryConfig,
    metadata::RegistryMetadata,
    package::{PackageRef, Version},
    registry::Registry,
    Error,
};

use crate::{
    source::{PackageSource, VersionInfo},
    Release,
};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WargRegistryMetadata {
    url: Option<String>,
}

pub struct WargSource {
    client: FileSystemClient,
}

impl WargSource {
    pub async fn new(
        registry: &Registry,
        registry_config: &RegistryConfig,
        registry_meta: &RegistryMetadata,
    ) -> Result<Self, Error> {
        let warg_meta = registry_meta
            .protocol_config::<WargRegistryMetadata>("warg")?
            .unwrap_or_default();
        let url = warg_meta.url.unwrap_or_else(|| registry.to_string());
        let WargConfig {
            client_config,
            auth_token,
        } = registry_config.try_into()?;

        let client_config = if let Some(client_config) = client_config {
            client_config
        } else {
            warg_client::Config::from_default_file()
                .map_err(Error::InvalidConfig)?
                .unwrap_or_default()
        };
        let client =
            FileSystemClient::new_with_config(Some(url.as_str()), &client_config, auth_token)
                .await
                .map_err(warg_registry_error)?;
        Ok(Self { client })
    }

    async fn fetch_package_info(&mut self, package: &PackageRef) -> Result<PackageInfo, Error> {
        let package_name = package_ref_to_name(package)?;
        self.client
            .package(&package_name)
            .await
            .map_err(warg_registry_error)
    }
}

#[async_trait]
impl PackageSource for WargSource {
    async fn list_all_versions(&mut self, package: &PackageRef) -> Result<Vec<VersionInfo>, Error> {
        let info = self.fetch_package_info(package).await?;
        Ok(info
            .state
            .releases()
            .map(|r| VersionInfo {
                version: r.version.clone(),
                yanked: r.yanked(),
            })
            .collect())
    }

    async fn get_release(
        &mut self,
        package: &PackageRef,
        version: &Version,
    ) -> Result<Release, Error> {
        let info = self.fetch_package_info(package).await?;
        let release = info
            .state
            .release(version)
            .ok_or_else(|| Error::VersionNotFound(version.clone()))?;
        let content_digest = release
            .content()
            .ok_or_else(|| Error::RegistryError(anyhow!("version {version} yanked")))?
            .to_string();
        Ok(Release {
            version: version.clone(),
            content_digest: content_digest.parse()?,
        })
    }

    async fn stream_content_unvalidated(
        &mut self,
        package: &PackageRef,
        release: &Release,
    ) -> Result<BoxStream<Result<Bytes, Error>>, Error> {
        self.stream_content(package, release).await
    }

    async fn stream_content(
        &mut self,
        package: &PackageRef,
        release: &Release,
    ) -> Result<BoxStream<Result<Bytes, Error>>, Error> {
        let package_name = package_ref_to_name(package)?;

        // warg client validates the digest matches the content
        let (_, stream) = self
            .client
            .download_exact_as_stream(&package_name, &release.version)
            .await
            .map_err(warg_registry_error)?;
        Ok(stream.map_err(Error::RegistryError).boxed())
    }
}

fn package_ref_to_name(package_ref: &PackageRef) -> Result<PackageName, Error> {
    PackageName::new(package_ref.to_string())
        .map_err(|err| Error::InvalidPackageRef(err.to_string()))
}

fn warg_registry_error(err: ClientError) -> Error {
    Error::RegistryError(err.into())
}
