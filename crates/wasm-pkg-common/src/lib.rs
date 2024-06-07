use http::uri::InvalidUri;

pub mod config;
pub mod label;
pub mod oci;
pub mod package;
pub mod registry;

use label::{InvalidLabel, Label};
use oci_distribution::errors::OciDistributionError;
pub use registry::Registry;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("error reading config file: {0}")]
    ConfigFileIoError(#[source] std::io::Error),
    #[error("failed to get registry credentials: {0:#}")]
    CredentialError(anyhow::Error),
    #[error("invalid content: {0}")]
    InvalidContent(String),
    #[error("invalid content digest: {0}")]
    InvalidContentDigest(String),
    #[error("invalid config: {0}")]
    InvalidConfig(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("invalid label: {0}")]
    InvalidLabel(#[from] InvalidLabel),
    #[error("invalid package manifest: {0}")]
    InvalidPackageManifest(String),
    #[error("invalid package pattern: {0}")]
    InvalidPackagePattern(String),
    #[error("invalid package ref: {0}")]
    InvalidPackageRef(String),
    #[error("invalid registry: {0}")]
    InvalidRegistry(#[from] InvalidUri),
    #[error("invalid registry metadata: {0}")]
    InvalidRegistryMetadata(#[source] serde_json::Error),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("no registry configured for namespace {0:?}")]
    NoRegistryForNamespace(Label),
    #[error("OCI error: {0}")]
    OciError(#[from] OciDistributionError),
    #[error("registry metadata fetch error: {0:#}")]
    RegistryMeta(#[source] anyhow::Error),
    #[error("invalid version: {0}")]
    VersionError(#[from] semver::Error),
    #[error("version not found: {0}")]
    VersionNotFound(semver::Version),
    #[error("version yanked: {0}")]
    VersionYanked(semver::Version),
    #[error("Warg error: {0}")]
    WargAnyhowError(#[source] anyhow::Error),
    #[error("Warg error: {0}")]
    WargError(#[from] warg_client::ClientError),
}

impl Error {
    fn invalid_config(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::InvalidConfig(Box::new(err))
    }
}
