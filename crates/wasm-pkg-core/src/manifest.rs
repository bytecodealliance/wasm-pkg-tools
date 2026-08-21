//! Type definitions and functions for working with `wkg.toml` files.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use semver::VersionReq;
use serde::{Deserialize, Serialize};
use wasm_pkg_common::package::{PackageRef, PackageSpec};
mod paths;
pub mod workspace;

use workspace::*;

use crate::manifest::paths::find_root_iter;

pub use crate::manifest::paths::find_root_manifest_for_wd;

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
        let manifest: Manifest = toml::from_str(contents)?;
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
            return Some(root);
        }
        None
    }

    // `Manifest` validations, mirrors cargo's `Workspace::validate`
    fn validate(&self) -> Result<()> {
        self.validate_workspace_exclusivity()?;
        self.validate_override_keys()?;
        // Add new validation rules with `self.validate_*()?;`
        Ok(())
    }

    /// Checks that override keys parse and that no package is covered by both a bare and a
    /// versioned key.
    ///
    /// Runs when a `wkg.toml` is loaded (see [`validate`](Self::validate)), and again when
    /// resolving, since a `Manifest` built directly in Rust code skips the load step.
    pub(crate) fn validate_override_keys(&self) -> Result<()> {
        let Some(overrides) = self.overrides.as_ref() else {
            return Ok(());
        };
        // `overrides` is a map, so walk it in a stable order
        let sorted_keys: BTreeSet<&str> = overrides.keys().map(String::as_str).collect();

        let mut bare: HashSet<PackageRef> = HashSet::new();
        let mut versioned: HashMap<PackageRef, Vec<&str>> = HashMap::new();
        for key in sorted_keys {
            let spec: PackageSpec = key
                .parse()
                .with_context(|| format!("invalid override key `{key}`"))?;
            match spec.version {
                Some(_) => versioned.entry(spec.package).or_default().push(key),
                None => {
                    bare.insert(spec.package);
                }
            }
        }

        let mut conflicts: Vec<String> = versioned
            .iter()
            .filter(|(package, _)| bare.contains(*package))
            .map(|(package, keys)| {
                format!(
                    "override `{package}` applies to every version of the package, so it \
                     conflicts with the versioned override(s) `{}`",
                    keys.join("`, `")
                )
            })
            .collect();
        if conflicts.is_empty() {
            return Ok(());
        }
        conflicts.sort_unstable();
        anyhow::bail!("{} - remove one or the other", conflicts.join("; "));
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
    // TODO(maktychev): reconcile load_from_path and load_root_workspace
    pub async fn load_root_workspace(cwd: &Path) -> Result<Option<WorkspaceRootConfig>> {
        let Some(manifest_file) = find_root_manifest_for_wd(cwd) else {
            return Ok(None);
        };
        let manifest_dir = manifest_file
            .parent()
            .context("unexpectedly missing directory containing manifest")?;
        let manifest = Self::load_from_path(&manifest_file).await?;

        if let Some(root) = manifest.root() {
            return Ok(Some(root.clone()));
        }

        // keep walking up if we have not found root
        for file in find_root_iter(&manifest_file) {
            let manifest = Self::load_from_path(&file).await?;
            if let Some(WorkspaceConfig::Root(root)) = manifest.workspace
                && root.is_explicitly_listed_member(manifest_dir)
            {
                return Ok(Some(root));
            }
        }

        Ok(None)
    }

    /// Serializes and writes the manifest to the given path.
    pub async fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        let contents = toml::to_string_pretty(self)?;
        tokio::fs::write(path, contents)
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

    #[test]
    fn override_keys_may_carry_a_version() {
        let manifest = Manifest::from_toml(
            r#"
[overrides]
"foo:bar@0.1.0" = { path = "bar-0.1.0" }
"foo:bar@0.2.0" = { path = "bar-0.2.0" }
"foo:baz" = { path = "baz" }
"#,
        )
        .expect("versioned override keys should be accepted");
        assert_eq!(manifest.overrides.unwrap().len(), 3);
    }

    #[test]
    fn override_keys_conflict_when_bare_and_versioned() {
        let err = Manifest::from_toml(
            r#"
[overrides]
"foo:bar" = { path = "bar" }
"foo:bar@0.1.0" = { path = "bar-0.1.0" }
"#,
        )
        .expect_err("a bare key alongside a versioned one is ambiguous");
        let err = format!("{err:#}");
        assert!(err.contains("foo:bar@0.1.0"), "unexpected error: {err}");
    }

    #[test]
    fn override_key_conflicts_are_all_reported_in_a_stable_order() {
        // Two conflicting packages: both must appear, and always in the same order, rather than
        // whichever the underlying map happened to yield first.
        let err = Manifest::from_toml(
            r#"
[overrides]
"zzz:two" = { path = "z" }
"zzz:two@0.2.0" = { path = "z2" }
"aaa:one" = { path = "a" }
"aaa:one@0.1.0" = { path = "a1" }
"#,
        )
        .expect_err("both packages conflict");
        let err = format!("{err:#}");
        let aaa = err.find("aaa:one").expect("aaa:one should be reported");
        let zzz = err.find("zzz:two").expect("zzz:two should be reported");
        assert!(aaa < zzz, "conflicts should be sorted: {err}");
    }

    #[test]
    fn override_keys_must_parse() {
        let err = Manifest::from_toml(
            r#"
[overrides]
"not a package ref" = { path = "bar" }
"#,
        )
        .expect_err("an unparseable override key should be rejected");
        let err = format!("{err:#}");
        assert!(
            err.contains("invalid override key"),
            "unexpected error: {err}"
        );
    }
}
