//! Warg package backend.

mod config;
mod loader;

use serde::Deserialize;
use warg_client::{storage::PackageInfo, ClientError, FileSystemClient};
use warg_protocol::registry::PackageName;
use wasm_pkg_common::{
    config::RegistryConfig, metadata::RegistryMetadata, package::PackageRef, registry::Registry,
    Error,
};

/// Re-exported for convenience.
pub use warg_client as client;

pub use config::WargRegistryConfig;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WargRegistryMetadata {
    url: Option<String>,
}

pub(crate) struct WargBackend {
    client: FileSystemClient,
}

impl WargBackend {
    pub async fn new(
        registry: &Registry,
        registry_config: &RegistryConfig,
        registry_meta: &RegistryMetadata,
    ) -> Result<Self, Error> {
        let warg_meta = registry_meta
            .protocol_config::<WargRegistryMetadata>("warg")?
            .unwrap_or_default();
        let url = warg_meta.url.unwrap_or_else(|| registry.to_string());
        let WargRegistryConfig {
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

    pub(crate) async fn fetch_package_info(
        &self,
        package: &PackageRef,
    ) -> Result<PackageInfo, Error> {
        let package_name = package_ref_to_name(package)?;
        self.client
            .package(&package_name)
            .await
            .map_err(warg_registry_error)
    }
}

pub(crate) fn package_ref_to_name(package_ref: &PackageRef) -> Result<PackageName, Error> {
    PackageName::new(package_ref.to_string())
        .map_err(|err| Error::InvalidPackageRef(err.to_string()))
}

pub(crate) fn warg_registry_error(err: ClientError) -> Error {
    match err {
        ClientError::PackageDoesNotExist { .. }
        | ClientError::PackageDoesNotExistWithHintHeader { .. } => Error::PackageNotFound,
        ClientError::PackageVersionDoesNotExist { version, .. } => Error::VersionNotFound(version),
        _ => Error::RegistryError(err.into()),
    }
}
