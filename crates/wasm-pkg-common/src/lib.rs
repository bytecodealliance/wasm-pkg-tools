use http::uri::InvalidUri;
use label::Label;

pub mod config;
pub mod label;
pub mod metadata;
pub mod package;
pub mod registry;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("error reading config file: {0}")]
    ConfigFileIoError(#[source] std::io::Error),
    #[error("failed to get registry credentials: {0:#}")]
    CredentialError(#[source] anyhow::Error),
    #[error("invalid config: {0}")]
    InvalidConfig(#[source] anyhow::Error),
    #[error("invalid content: {0}")]
    InvalidContent(String),
    #[error("invalid content digest: {0}")]
    InvalidContentDigest(String),
    #[error("invalid package manifest: {0}")]
    InvalidPackageManifest(String),
    #[error("invalid package pattern: {0}")]
    InvalidPackagePattern(String),
    #[error("invalid label: {0}")]
    InvalidLabel(#[from] label::InvalidLabel),
    #[error("invalid package ref: {0}")]
    InvalidPackageRef(String),
    #[error("invalid registry: {0}")]
    InvalidRegistry(#[from] InvalidUri),
    #[error("invalid registry metadata: {0}")]
    InvalidRegistryMetadata(#[source] anyhow::Error),
    #[error("invalid version: {0}")]
    InvalidVersion(#[from] semver::Error),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("no registry configured for namespace {0:?}")]
    NoRegistryForNamespace(Label),
    #[error("registry error: {0}")]
    RegistryError(#[source] anyhow::Error),
    #[error("registry metadata error: {0:#}")]
    RegistryMetadataError(#[source] anyhow::Error),
    #[error("version not found: {0}")]
    VersionNotFound(semver::Version),
}

impl Error {
    fn invalid_config(err: impl Into<anyhow::Error>) -> Self {
        Self::InvalidConfig(err.into())
    }
}
