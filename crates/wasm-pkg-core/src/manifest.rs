//! Type definitions and functions for working with `wkg.toml` files.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

/// The default name of the manifest file.
pub const MANIFEST_FILE_NAME: &str = "wkg.toml";

/// The structure for a wkg.toml manifest file. This file is entirely optional and is used for
/// overriding and annotating wasm packages.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Overrides for various packages
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<HashMap<String, Override>>,
    /// Additional metadata about the package. This will override any metadata already set by other
    /// tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

impl Manifest {
    /// Loads a manifest file from the given path.
    pub async fn load_from_path(path: impl AsRef<Path>) -> Result<Manifest> {
        tracing::info!(path = %path.as_ref().display(), "loading wkg manifest file");
        let contents = tokio::fs::read_to_string(path)
            .await
            .context("unable to load manifest from file")?;
        let manifest: Manifest =
            toml::from_str(&contents).context("unable to parse manifest file")?;
        Ok(manifest)
    }

    /// Attempts to load the manifest from the current directory. Most of the time, users of this
    /// crate should use this function. Right now it just checks for a `wkg.toml` file in the current
    /// directory, but we could add more resolution logic in the future. If the file is not found, a
    /// default empty manifest is returned.
    pub async fn load() -> Result<Manifest> {
        let manifest_path = PathBuf::from(MANIFEST_FILE_NAME);
        if !tokio::fs::try_exists(&manifest_path).await? {
            return Ok(Manifest::default());
        }
        Self::load_from_path(manifest_path).await
    }

    /// Serializes and writes the manifest to the given path.
    pub async fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        let contents = toml::to_string_pretty(self)?;
        let mut file = tokio::fs::File::create(path).await?;
        file.write_all(contents.as_bytes())
            .await
            .context("unable to write manifest to path")
    }

    /// Returns a matching override name and value for the input path
    pub(crate) fn has_override(&self, path: impl AsRef<Path>) -> bool {
        let path = path.as_ref().canonicalize().ok();
        self.overrides
            .iter()
            .flat_map(|map| map.iter())
            .find(|(_, o)| o.path.as_ref().and_then(|p| p.canonicalize().ok()) == path)
            .is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct Metadata {
    /// The author(s) of the package. Alias supports prior definition as `author`.
    /// Note that unlike in a Cargo.toml, this authors is a string, not a list of string.
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "author")]
    pub authors: Option<String>,
    /// The package description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The package license.
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "license")]
    pub licenses: Option<String>,
    /// The package source code URL.
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "repository")]
    pub source: Option<String>,
    /// The package homepage URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// The package source control revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_roundtrip() {
        let tempdir = tempfile::tempdir().unwrap();
        let manifest_path = tempdir.path().join(MANIFEST_FILE_NAME);
        let manifest = Manifest {
            overrides: Some(HashMap::from([(
                "foo:bar".to_string(),
                Override {
                    path: Some(PathBuf::from("bar")),
                    version: Some(VersionReq::parse("1.0.0").unwrap()),
                },
            )])),
            metadata: Some(Metadata {
                authors: Some("Foo Bar".to_string()),
                description: Some("Foobar baz".to_string()),
                licenses: Some("FBB".to_string()),
                source: Some("https://gitfoo/bar".to_string()),
                homepage: Some("https://foo.bar".to_string()),
                revision: Some("f00ba4".to_string()),
            }),
        };

        manifest
            .write(&manifest_path)
            .await
            .expect("unable to write manifest");
        let loaded_manifest = Manifest::load_from_path(manifest_path)
            .await
            .expect("unable to load manifest");
        assert_eq!(
            manifest, loaded_manifest,
            "manifest loaded from file does not match original manifest"
        );
    }
}
