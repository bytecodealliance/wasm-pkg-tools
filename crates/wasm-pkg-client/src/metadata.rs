use anyhow::Context;
use reqwest::StatusCode;
use wasm_pkg_common::{
    Error,
    metadata::{REGISTRY_METADATA_PATH, RegistryMetadata},
    registry::Registry,
};

/// Extension trait for [`RegistryMetadata`] adding client functionality.
pub trait RegistryMetadataExt: Sized {
    /// Attempt to fetch [`RegistryMetadata`] from the given [`Registry`]. On
    /// failure, return defaults.
    fn fetch_or_default(registry: &Registry) -> impl std::future::Future<Output = Self> + Send;

    /// Fetch [`RegistryMetadata`] from the given [`Registry`].
    fn fetch(
        registry: &Registry,
    ) -> impl std::future::Future<Output = Result<Option<Self>, Error>> + Send;
}

impl RegistryMetadataExt for RegistryMetadata {
    async fn fetch_or_default(registry: &Registry) -> Self {
        match Self::fetch(registry).await {
            Ok(Some(meta)) => {
                tracing::debug!(?meta, "Got registry metadata");
                meta
            }
            Ok(None) => {
                tracing::debug!("Metadata not found");
                Default::default()
            }
            Err(err) => {
                tracing::warn!(error = ?err, "Error fetching registry metadata");
                Default::default()
            }
        }
    }

    async fn fetch(registry: &Registry) -> Result<Option<Self>, Error> {
        // TODO use `core::net::Ipv4Addr::is_loopback()`
        let scheme = if registry.host() == "localhost" || registry.host() == "127.0.0.1" {
            "http"
        } else {
            "https"
        };
        let url = format!("{scheme}://{registry}{REGISTRY_METADATA_PATH}");
        fetch_url(&url)
            .await
            .with_context(|| format!("error fetching registry metadata from {url:?}"))
            .map_err(Error::RegistryMetadataError)
    }
}

async fn fetch_url(url: &str) -> anyhow::Result<Option<RegistryMetadata>> {
    tracing::debug!(?url, "Fetching registry metadata");

    let resp = reqwest::get(url).await?;
    if resp.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let resp = resp.error_for_status()?;
    Ok(Some(resp.json().await?))
}
