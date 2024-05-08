use anyhow::Context;
use reqwest::StatusCode;
use serde::Deserialize;

use crate::Error;

const WELL_KNOWN_PATH: &str = ".well-known/warg/registry.json";

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryMeta {
    pub oci_registry: Option<String>,
    pub oci_namespace_prefix: Option<String>,
    pub warg_url: Option<String>,
}

impl RegistryMeta {
    pub async fn fetch_or_default(domain: &str) -> Self {
        match Self::fetch(domain).await {
            Ok(Some(meta)) => {
                tracing::debug!("Got registry metadata {meta:?}");
                meta
            }
            Ok(None) => {
                tracing::debug!("Metadata not found");
                Default::default()
            }
            Err(err) => {
                tracing::warn!("Error fetching registry metadata: {err}");
                Default::default()
            }
        }
    }

    pub async fn fetch(domain: &str) -> Result<Option<Self>, Error> {
        let scheme = if domain.starts_with("localhost:") {
            "http"
        } else {
            "https"
        };
        let url = format!("{scheme}://{domain}/{WELL_KNOWN_PATH}");
        Self::fetch_url(&url)
            .await
            .with_context(|| format!("error fetching registry metadata from {url:?}"))
            .map_err(Error::RegistryMeta)
    }

    async fn fetch_url(url: &str) -> anyhow::Result<Option<Self>> {
        tracing::debug!("Fetching registry metadata from {url:?}");
        let resp = reqwest::get(url).await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = resp.error_for_status()?;
        Ok(Some(resp.json().await?))
    }
}
