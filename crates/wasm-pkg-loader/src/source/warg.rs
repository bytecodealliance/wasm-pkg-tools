use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{stream::BoxStream, StreamExt, TryStreamExt};
use secrecy::SecretString;
use semver::Version;
use warg_client::{storage::PackageInfo, ClientError, FileSystemClient};
use warg_protocol::registry::PackageName;

use crate::{
    meta::RegistryMeta,
    source::{PackageSource, VersionInfo},
    Error, PackageRef, Release,
};

#[derive(Clone, Debug, Default)]
pub struct WargConfig {
    pub client_config: Option<warg_client::Config>,
    pub auth_token: Option<SecretString>,
}

pub struct WargSource {
    client: FileSystemClient,
}

impl WargSource {
    pub async fn new(
        registry: String,
        config: WargConfig,
        registry_meta: RegistryMeta,
    ) -> Result<Self, Error> {
        let url = registry_meta.warg_url.unwrap_or(registry);
        let WargConfig {
            client_config,
            auth_token,
        } = config;

        let client_config = if let Some(client_config) = client_config {
            client_config
        } else {
            warg_client::Config::from_default_file()
                .map_err(Error::InvalidConfig)?
                .unwrap_or_default()
        };
        let client =
            FileSystemClient::new_with_config(Some(url.as_str()), &client_config, auth_token).await?;
        Ok(Self { client })
    }

    async fn fetch_package_info(&mut self, package: &PackageRef) -> Result<PackageInfo, Error> {
        let package_name = package.try_into()?;
        Ok(self.client.package(&package_name).await?)
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
            .ok_or_else(|| Error::VersionYanked(version.clone()))?
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
        let package_name = package.try_into()?;

        // warg client validates the digest matches the content
        let (_, stream) = self
            .client
            .download_exact_as_stream(&package_name, &release.version)
            .await?;
        Ok(stream.map_err(Into::into).boxed())
    }
}

impl TryFrom<&PackageRef> for PackageName {
    type Error = Error;

    fn try_from(value: &PackageRef) -> Result<Self, Self::Error> {
        Self::new(value.to_string()).map_err(|err| Error::WargError(ClientError::Other(err)))
    }
}
