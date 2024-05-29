use http::uri::InvalidUri;

pub mod config;
mod label;
mod package;
pub mod registry;

use label::InvalidLabel;
pub use registry::Registry;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("error reading config file: {0}")]
    ConfigFileIoError(#[source] std::io::Error),
    #[error("invalid config: {0}")]
    InvalidConfig(#[source] Box<dyn std::error::Error>),
    #[error("invalid package pattern: {0}")]
    InvalidPackagePattern(String),
    #[error("invalid label: {0}")]
    InvalidLabel(#[from] InvalidLabel),
    #[error("invalid package ref: {0}")]
    InvalidPackageRef(String),
    #[error("invalid registry: {0}")]
    InvalidRegistry(#[from] InvalidUri),
    #[error("invalid registry metadata: {0}")]
    InvalidRegistryMetadata(#[source] serde_json::Error),
}

impl Error {
    fn invalid_config(err: impl std::error::Error + 'static) -> Self {
        Self::InvalidConfig(Box::new(err))
    }
}
