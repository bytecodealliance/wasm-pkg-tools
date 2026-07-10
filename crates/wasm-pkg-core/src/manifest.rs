//! Type definitions and functions for working with `wkg.toml` files.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

mod paths;
pub mod workspace;

use workspace::*;

use crate::manifest::paths::{find_root_iter, find_root_manifest_for_wd};

/// The default name of the manifest file.
pub const MANIFEST_FILE_NAME: &str = "wkg.toml";
/// Directory next to the root [`MANIFEST_FILE_NAME`] that holds multi-package `deps` and `config.toml`.
pub const WORKSPACE_OUT_DIR: &str = "wkg";

/// The structure for a wkg.toml manifest file. This file is entirely optional and is used for
/// overriding and annotating wasm packages.
/// `workspace` is mutually exclusive with `overrides` and top-level `metadata`
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Workspace declaration.
    // TODO: this should be a `TomlWorkspace` so that serialization is not coupled to the config structure
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceConfig>,
    /// Overrides for various packages
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<HashMap<String, Override>>,
    /// Additional metadata about the package. This will override any metadata already set by other
    /// tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

impl Manifest {
    fn from_toml(contents: &str) -> Result<Manifest> {
        let manifest: Manifest =
            toml::from_str(contents).context("unable to parse manifest file")?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Loads a manifest file from the given path.
    pub async fn load_from_path(path: impl AsRef<Path>) -> Result<Manifest> {
        let path = path.as_ref();
        tracing::info!(path = %path.display(), "loading wkg manifest file");
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("unable to load manifest from {}", path.display()))?;
        let mut manifest = Self::from_toml(&contents)
            .with_context(|| format!("invalid manifest at {}", path.display()))?;
        if let Some(WorkspaceConfig::Root(root)) = &mut manifest.workspace {
            root.root_dir = path
                .parent()
                .with_context(|| {
                    format!("manifest path has no parent directory: {}", path.display())
                })?
                .to_path_buf();
            // Resolve globs and relative paths eagerly
            root.members = WorkspaceRootConfig::resolve_members(&root.members, &root.root_dir);
        }
        Ok(manifest)
    }

    fn root(&self) -> Option<&WorkspaceRootConfig> {
        if let Some(WorkspaceConfig::Root(root)) = &self.workspace {
            return Some(&root);
        }
        None
    }

    // `Manifest` validations, mirrors cargo's `Workspace::validate`
    fn validate(&self) -> Result<()> {
        self.validate_workspace_exclusivity()?;
        // Add new validation rules with `self.validate_*()?;`
        Ok(())
    }

    // no overrides or top-level metadata when workspace is present
    fn validate_workspace_exclusivity(&self) -> Result<()> {
        if self.workspace.is_none() {
            return Ok(());
        }
        let mut conflicts = Vec::new();
        if self.overrides.is_some() {
            conflicts.push("overrides");
        }
        if self.metadata.is_some() {
            conflicts.push("metadata");
        }
        if conflicts.is_empty() {
            return Ok(());
        }
        anyhow::bail!(
            "`[workspace]` cannot coexist with: `[{}]` - \
             use `[workspace.metadata]` for workspace level values",
            conflicts.join("]`, `[")
        );
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

    /// Tries to find the root workspace config
    /// Returns `Ok(None)` when there is no `wkg.toml` ancestor that can be [`WorkspaceRootConfig`]
    pub async fn load_root_workspace(cwd: &Path) -> Result<Option<WorkspaceRootConfig>> {
        let Some(manifest_file) = find_root_manifest_for_wd(cwd) else {
            return Ok(None);
        };
        let manifest_dir = manifest_file.parent().unwrap();
        let manifest = Self::load_from_path(&manifest_file).await?;

        if let Some(root) = manifest.root() {
            return Ok(Some(root.clone()));
        }

        // keep walking up if we have not found root
        for file in find_root_iter(&manifest_file) {
            let manifest = Self::load_from_path(&file).await?;
            if let Some(WorkspaceConfig::Root(root)) = manifest.workspace {
                if root.is_explicitly_listed_member(&manifest_dir) {
                    return Ok(Some(root));
                }
            }
        }

        Ok(None)
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
            workspace: None,
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
