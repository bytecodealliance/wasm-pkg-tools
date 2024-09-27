//! Type definitions and functions for working with `wkg.toml` files.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use wasm_metadata::{Link, LinkType, RegistryMetadata};

/// The default name of the configuration file.
pub const CONFIG_FILE_NAME: &str = "wkg.toml";

/// The structure for a wkg.toml configuration file. This file is entirely optional and is used for
/// overriding and annotating wasm packages.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Config {
    /// Overrides for various packages
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<HashMap<String, Override>>,
    /// Additional metadata about the package. This will override any metadata already set by other
    /// tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

impl Config {
    /// Loads a configuration file from the given path.
    pub async fn load_from_path(path: impl AsRef<Path>) -> Result<Config> {
        let contents = tokio::fs::read_to_string(path)
            .await
            .context("unable to load config from file")?;
        let config: Config = toml::from_str(&contents).context("unable to parse config file")?;
        Ok(config)
    }

    /// Attempts to load the configuration from the current directory. Most of the time, users of this
    /// crate should use this function. Right now it just checks for a `wkg.toml` file in the current
    /// directory, but we could add more resolution logic in the future. If the file is not found, a
    /// default empty config is returned.
    pub async fn load() -> Result<Config> {
        let config_path = PathBuf::from(CONFIG_FILE_NAME);
        if !tokio::fs::try_exists(&config_path).await? {
            return Ok(Config::default());
        }
        Self::load_from_path(config_path).await
    }

    /// Serializes and writes the configuration to the given path.
    pub async fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        let contents = toml::to_string_pretty(self)?;
        let mut file = tokio::fs::File::create(path).await?;
        file.write_all(contents.as_bytes())
            .await
            .context("unable to write config to path")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Override {
    /// A path to the package on disk. If this is set, the package will be loaded from the given
    /// path. If this is not set, the package will be loaded from the registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// Overrides the version of a package specified in a world file. This is for advanced use only
    /// and may break things.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<VersionReq>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Metadata {
    /// The authors of the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<String>>,
    /// The categories of the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<String>>,
    /// The package description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The package license.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// The package documentation URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    /// The package homepage URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// The package repository URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
}

impl From<Metadata> for wasm_metadata::RegistryMetadata {
    fn from(value: Metadata) -> Self {
        let mut meta = RegistryMetadata::default();
        meta.set_authors(value.authors);
        meta.set_categories(value.categories);
        meta.set_description(value.description);
        meta.set_license(value.license);
        let mut links = Vec::new();
        if let Some(documentation) = value.documentation {
            links.push(Link {
                ty: LinkType::Documentation,
                value: documentation,
            });
        }
        if let Some(homepage) = value.homepage {
            links.push(Link {
                ty: LinkType::Homepage,
                value: homepage,
            });
        }
        if let Some(repository) = value.repository {
            links.push(Link {
                ty: LinkType::Repository,
                value: repository,
            });
        }
        meta.set_links((!links.is_empty()).then_some(links));
        meta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_roundtrip() {
        let tempdir = tempfile::tempdir().unwrap();
        let config_path = tempdir.path().join(CONFIG_FILE_NAME);
        let config = Config {
            overrides: Some(HashMap::from([(
                "foo:bar".to_string(),
                Override {
                    path: Some(PathBuf::from("bar")),
                    version: Some(VersionReq::parse("1.0.0").unwrap()),
                },
            )])),
            metadata: Some(Metadata {
                authors: Some(vec!["foo".to_string(), "bar".to_string()]),
                categories: Some(vec!["foo".to_string(), "bar".to_string()]),
                description: Some("foo".to_string()),
                license: Some("foo".to_string()),
                documentation: Some("foo".to_string()),
                homepage: Some("foo".to_string()),
                repository: Some("foo".to_string()),
            }),
        };

        config
            .write(&config_path)
            .await
            .expect("unable to write config");
        let loaded_config = Config::load_from_path(config_path)
            .await
            .expect("unable to load config");
        assert_eq!(
            config, loaded_config,
            "config loaded from file does not match original config"
        );
    }
}
