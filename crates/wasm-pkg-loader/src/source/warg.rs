use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{stream::BoxStream, StreamExt, TryStreamExt};
use secrecy::SecretString;
use semver::Version;
use warg_client::{
    storage::{PackageInfo, RegistryStorage},
    ClientError, FileSystemClient,
};
use warg_protocol::registry::PackageName;

use crate::{meta::RegistryMeta, source::PackageSource, Error, PackageRef, Release};

#[derive(Clone, Debug, Default)]
pub struct WargConfig {
    pub client_config: warg_client::Config,
    pub auth_token: Option<SecretString>,
}

pub struct WargSource {
    client: FileSystemClient,
    api_client: warg_client::api::Client,
}

impl WargSource {
    pub fn new(
        registry: String,
        config: WargConfig,
        registry_meta: RegistryMeta,
    ) -> Result<Self, Error> {
        let url = registry_meta.warg_url.unwrap_or(registry);
        let client = FileSystemClient::new_with_config(
            Some(url.as_str()),
            &config.client_config,
            config.auth_token.clone(),
        )?;
        let api_client = warg_client::api::Client::new(client.url().to_string(), config.auth_token)
            .map_err(ClientError::Other)?;
        Ok(Self { client, api_client })
    }

    async fn fetch_package_info(&mut self, package: &PackageRef) -> Result<PackageInfo, Error> {
        let package_name = package.try_into()?;
        self.client.upsert([&package_name]).await?;
        self.client
            .registry()
            .load_package(self.client.get_warg_registry(), &package_name)
            .await
            .map_err(ClientError::Other)?
            .ok_or_else(|| {
                // TODO: standardize this error
                ClientError::Other(anyhow::anyhow!("package not found")).into()
            })
    }
}

#[async_trait]
impl PackageSource for WargSource {
    async fn list_all_versions(&mut self, package: &PackageRef) -> Result<Vec<Version>, Error> {
        let info = self.fetch_package_info(package).await?;
        Ok(info
            .state
            .releases()
            .map(|release| release.version.clone())
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
            // TODO: standardize this error
            .ok_or_else(|| ClientError::Other(anyhow::anyhow!("version not found")))?;
        let content_digest = release
            .content()
            .ok_or_else(|| ClientError::Other(anyhow::anyhow!("release has been yanked")))?
            .to_string();
        Ok(Release {
            version: version.clone(),
            content_digest: content_digest.parse()?,
        })
    }

    async fn stream_content_unvalidated(
        &mut self,
        _package: &PackageRef,
        release: &Release,
    ) -> Result<BoxStream<Result<Bytes, Error>>, Error> {
        let digest = release
            .content_digest
            .to_string()
            .parse()
            .map_err(|err| ClientError::Other(anyhow::anyhow!("{err}")))?;
        let stream = self
            .api_client
            .download_content(&digest)
            .await
            .map_err(ClientError::Api)?;
        Ok(stream
            .map_err(|err| Error::WargError(ClientError::Other(err)))
            .boxed())
    }
}

impl TryFrom<&PackageRef> for PackageName {
    type Error = Error;

    fn try_from(value: &PackageRef) -> Result<Self, Self::Error> {
        Self::new(value.to_string()).map_err(|err| Error::WargError(ClientError::Other(err)))
    }
}
