use crate::manifest::paths::{expand_globs, normalize_path};

use super::*;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `[workspace]` table inside a `wkg.toml` manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum WorkspaceConfig {
    Root(WorkspaceRootConfig),
    // - `workspace = "path/to/workspace/root"`
    // - TODO(mkatychev): implement workspace package level field
    // - https://doc.rust-lang.org/cargo/reference/manifest.html#the-workspace-field
    Member { root: Option<String> },
}
impl Default for WorkspaceConfig {
    fn default() -> Self {
        // TODO(mkatychev): use member variant instead
        Self::Root(WorkspaceRootConfig::default())
    }
}

impl WorkspaceConfig {
    /// Resolves the [`WorkspaceRootConfig`] for a given [`Self`]
    pub fn as_root(&self) -> Option<&WorkspaceRootConfig> {
        match self {
            WorkspaceConfig::Root(r) => Some(r),
            WorkspaceConfig::Member { .. } => None,
        }
    }
}

/// Intermediate configuration of a workspace root in a manifest.
///
/// Knows the Workspace Root path, as well as `members` and `metadata`, which
/// together tell if some path is recognized as a member by this root or not.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct WorkspaceRootConfig {
    #[serde(default, skip)]
    pub(super) root_dir: PathBuf,
    pub members: Vec<PathBuf>,
    pub metadata: Option<Metadata>,
}

impl WorkspaceRootConfig {
    /// Directory containing the workspace-root `wkg.toml`.
    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    /// Directory containing workspace level `deps` `and config.toml`
    pub fn out_dir(&self) -> PathBuf {
        self.root_dir.join(PathBuf::from(WORKSPACE_OUT_DIR))
    }

    /// Checks if the path is explicitly listed as a workspace member.
    ///
    /// Returns `true` ONLY if:
    /// - The path is the workspace root manifest itself, or
    /// - The path matches one of the explicit `members` patterns
    ///
    /// NOTE: This does NOT check for implicit path dependency membership.
    /// A `false` return does NOT mean the package is definitely not a member -
    /// it could still be a member via path dependencies. Callers should fallback
    /// to full workspace loading when this returns `false`.
    // FIXME: implement cargo WorkspaceRootConfig::{member_paths, expand_member_path}
    // this should not be eagerly evaluated
    pub(super) fn is_explicitly_listed_member(&self, manifest_path: &Path) -> bool {
        let root_manifest = self.root_dir.join("Cargo.toml");
        if manifest_path == root_manifest {
            return true;
        }

        let manifest_path = normalize_path(manifest_path);
        self.members.iter().any(|member| member == &manifest_path)
    }
    // Expand globs and relative paths
    pub(super) fn resolve_members(members: &[PathBuf], root_dir: &Path) -> Vec<PathBuf> {
        members
            .iter()
            .flat_map(|entry| expand_globs(entry, root_dir))
            .flatten()
            .map(|p| normalize_path(&p))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    // NOTE: tree dir comments generated using `eza -T dir/`
    fn touch(path: impl AsRef<Path>) {
        let path = path.as_ref();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    #[test]
    fn find_root_manifest_for_wd_first_ancestor() {
        // dir
        // ├── wkg.toml
        // └── pkg
        //     └── wkg.toml
        let tempdir = tempfile::tempdir().unwrap();
        let root_dir = tempdir.path();
        touch(root_dir.join("wkg.toml"));
        touch(root_dir.join("pkg/wkg.toml"));
        fs::create_dir_all(root_dir.join("pkg/wit")).unwrap();

        let found = find_root_manifest_for_wd(root_dir.join("pkg/wit")).unwrap();
        assert_eq!(found, root_dir.join("pkg/wkg.toml"));
    }

    #[test]
    fn find_root_manifest_for_wd_none() {
        let tempdir = tempfile::tempdir().unwrap();
        assert!(find_root_manifest_for_wd(tempdir.path()).is_none());
    }

    #[test]
    fn find_root_iter_ancestors() {
        // dir
        // ├── wkg.toml
        // └── pkg
        //     └── wkg.toml
        let tempdir = tempfile::tempdir().unwrap();
        let root_dir = tempdir.path();
        touch(root_dir.join("wkg.toml"));
        let member = root_dir.join("pkg/wkg.toml");
        touch(&member);

        let roots: Vec<PathBuf> = find_root_iter(&member).collect();
        assert_eq!(roots, vec![root_dir.join("wkg.toml")]);
    }

    #[test]
    fn resolve_members_skip_non_dirs() {
        // dir
        // ├── example-a
        // │   └── wit
        // │       └── pkg.wit
        // ├── example-b
        // │   └── wit
        // │       └── pkg.wit
        // ├── example-c
        // │   └── wit
        // │       └── pkg.wit
        // └── example-d
        let tempdir = tempfile::tempdir().unwrap();
        let root_dir = tempdir.path();
        let member_wit = ["example-a/wit", "example-b/wit", "example-c/wit"];
        for dir in member_wit {
            fs::create_dir_all(root_dir.join(dir)).unwrap();
            fs::write(root_dir.join(dir).join("pkg.wit"), "").unwrap();
        }
        fs::write(root_dir.join("example-d"), "").unwrap();

        let members = vec![PathBuf::from("example-*/wit")];
        let mut result = WorkspaceRootConfig::resolve_members(&members, root_dir);
        result.sort();
        let mut expected: Vec<_> = member_wit.iter().map(|p| root_dir.join(p)).collect();
        expected.sort();
        assert_eq!(result, expected, "glob should expand to matching dirs only");
    }

    // skip directories without `.wit` files
    #[test]
    fn resolve_members_skip_non_wit_dirs() {
        // dir
        // └── pkg
        //     ├── core
        //     │   └── types.wit
        //     └── filesystem
        let tempdir = tempfile::tempdir().unwrap();
        let root_dir = tempdir.path();
        let core = root_dir.join("pkg/core");
        let filesystem = root_dir.join("pkg/filesystem");
        fs::create_dir_all(&core).unwrap();
        fs::write(core.join("types.wit"), "package foo:core;\n").unwrap();
        fs::create_dir_all(filesystem).unwrap();

        let members = vec![PathBuf::from("pkg/*")];
        let result = WorkspaceRootConfig::resolve_members(&members, root_dir);
        assert_eq!(result, vec![core]);
    }

    #[test]
    fn resolved_members_keeps_literal_even_when_empty() {
        let tempdir = tempfile::tempdir().unwrap();
        let root_dir = tempdir.path();
        // dir
        // └── empty-pkg
        fs::create_dir_all(root_dir.join("empty-pkg")).unwrap();

        let members = vec![PathBuf::from("empty-pkg")];
        let result = WorkspaceRootConfig::resolve_members(&members, root_dir);
        assert_eq!(result, vec![root_dir.join("empty-pkg")]);
    }

    #[test]
    fn workspace_root() {
        let toml = r#"
[workspace]
members = ["a/wit", "b/wit"]
"#;
        let cfg = Manifest::from_toml(toml).unwrap();
        let root = cfg.workspace.unwrap();
        assert_eq!(
            root.as_root().unwrap().members,
            vec![PathBuf::from("a/wit"), PathBuf::from("b/wit")],
        );
        assert!(cfg.overrides.is_none() && cfg.metadata.is_none());
    }

    #[test]
    fn workspace_metadata() {
        let toml = r#"
[workspace]
members = ["a/wit"]

[workspace.metadata]
authors = "Webster Assembler"
"#;
        let cfg = Manifest::from_toml(toml).unwrap();
        let root = cfg.root().unwrap();
        let meta = root.metadata.as_ref().unwrap();
        assert_eq!(meta.authors.as_ref().unwrap(), "Webster Assembler");
    }

    #[test]
    fn workspace_overrides_incompatible() {
        let toml = r#"
[workspace]
members = ["a/wit"]

[overrides."foo:bar"]
path = "oo-bar"
"#;
        Manifest::from_toml(toml).unwrap_err();
    }

    fn write(path: impl AsRef<Path>, contents: &str) {
        let path = path.as_ref();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[tokio::test]
    async fn load_workspace_from_root_and_member() {
        let tempdir = tempfile::tempdir().unwrap();
        let root_dir = tempdir.path();
        // dir
        // ├── wkg.toml
        // └── pkg-a
        //     └── wit
        write(
            root_dir.join("wkg.toml"),
            r#"
[workspace]
members = ["pkg-a"]
"#,
        );
        fs::create_dir_all(root_dir.join("pkg-a/wit")).unwrap();

        let expected = root_dir.canonicalize().unwrap();
        let from_root = Manifest::load_root_workspace(root_dir)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(from_root.root_dir.canonicalize().unwrap(), expected);

        let from_member = Manifest::load_root_workspace(&root_dir.join("pkg-a/wit"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(from_member.root_dir.canonicalize().unwrap(), expected);
    }

    #[tokio::test]
    async fn load_workspace_no_root() {
        // Standalone: wkg.toml exists but has no [workspace] table -> None.
        let tempdir = tempfile::tempdir().unwrap();
        let root_dir = tempdir.path();
        write(
            root_dir.join("wkg.toml"),
            r#"
[metadata]
authors = "Webster Assembler"
"#,
        );
        assert!(
            Manifest::load_root_workspace(root_dir)
                .await
                .unwrap()
                .is_none()
        );
    }
}
