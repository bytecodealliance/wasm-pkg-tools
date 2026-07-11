//! Args and commands for interacting with WIT files and dependencies
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anstream::eprintln;
use anyhow::Context;
use clap::{Args, Subcommand};
use tempfile::NamedTempFile;
use wasm_pkg_client::caching::{CachingClient, FileCache};
use wasm_pkg_common::package::{PackageRef, Version};
use wasm_pkg_core::wit::WIT_DEPS_DIR;
use wasm_pkg_core::{
    lock::{LOCK_FILE_NAME, LockFile, LockedPackage},
    manifest::{MANIFEST_FILE_NAME, Manifest, workspace::WorkspaceRootConfig},
    resolver::DependencyResolutionMap,
    wit::{self, OutputType},
};

use crate::Common;
use crate::overlay::PublishVerifier;

/// Commands for interacting with wit
#[derive(Debug, Subcommand)]
pub enum WitCommands {
    Build(BuildArgs),
    Fetch(FetchArgs),
    Update(UpdateArgs),
}

impl WitCommands {
    pub async fn run(self) -> anyhow::Result<()> {
        match self {
            WitCommands::Build(args) => args.run().await,
            WitCommands::Fetch(args) => args.run().await,
            WitCommands::Update(args) => args.run().await,
        }
    }
}

/// Build a WIT package from a directory. By default, this will fetch all dependencies needed
/// and encode them in the WIT package. This will generate a lock file that can be used to fetch
/// the dependencies in the future.
#[derive(Debug, Args)]
pub struct BuildArgs {
    /// The directory containing the WIT files to build.
    #[clap(short = 'd', long = "wit-dir", default_value = "wit")]
    pub dir: PathBuf,

    /// The name of the file that should be written. This can also be a full path. Defaults to the
    /// current directory with the name of the package.
    #[clap(short = 'o', long = "output")]
    pub output: Option<PathBuf>,

    #[clap(flatten)]
    pub common: Common,
}

/// Fetch dependencies for a component. This will read the package containing the world(s) you
/// have defined in the given wit directory (`wit` by default). It will then fetch the
/// dependencies and write them to the `deps` directory along with a lock file. If no lock file
/// exists, it will fetch all dependencies. If a lock file exists, it will fetch any
/// dependencies that are not in the lock file and update the lock file. To update the lock
/// file, use the `update` command.
#[derive(Debug, Args)]
pub struct FetchArgs {
    /// The directory containing the WIT files to fetch dependencies for.
    /// Falls back to workspace manifest if empty.
    pub dir: Option<PathBuf>,

    /// The desired output type of the dependencies. Valid options are "wit" or "wasm" (wasm is the
    /// WIT package binary format).
    #[clap(short = 't', long = "type")]
    pub output_type: Option<OutputType>,

    #[clap(flatten)]
    pub common: Common,
}

/// Update the lock file with the latest dependencies. This will update all dependencies and
/// generate a new lock file.
#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// The directory containing the WIT files to update dependencies for.
    #[clap(short = 'd', long = "wit-dir", default_value = "wit")]
    pub dir: PathBuf,

    /// The desired output type of the dependencies. Valid options are "wit" or "wasm" (wasm is the
    /// WIT package binary format).
    #[clap(short = 't', long = "type")]
    pub output_type: Option<OutputType>,

    #[clap(flatten)]
    pub common: Common,
}

impl BuildArgs {
    pub async fn run(self) -> anyhow::Result<()> {
        let client = self.common.get_client().await?;
        let mut lock_file = LockFile::load(false).await?;
        let (pkg_ref, version, bytes) = build_wit_dir(&self.dir, client, &mut lock_file).await?;
        let output_path = if let Some(path) = self.output {
            path
        } else {
            let mut file_name = pkg_ref.to_string();
            if let Some(version) = version {
                file_name.push_str(&format!("@{version}"));
            }
            file_name.push_str(".wasm");
            PathBuf::from(file_name)
        };

        tokio::fs::write(&output_path, bytes).await?;
        // Now write out the lock file since everything else succeeded
        lock_file.write().await?;
        eprintln!("WIT package written to {}", output_path.display());
        Ok(())
    }
}

/// Build a WIT package from a directory, returning the resolved package ref, optional
/// version, and the encoded component bytes.
pub async fn build_wit_dir(
    dir: impl AsRef<Path>,
    client: CachingClient<FileCache>,
    lock_file: &mut LockFile,
) -> anyhow::Result<(PackageRef, Option<Version>, Vec<u8>)> {
    check_dir(&dir).await?;
    let manifest = Manifest::load().await?;
    let result = wit::build_package(&manifest, dir.as_ref(), lock_file, client)
        .await
        .with_context(|| format!("failed to build WIT directory `{}`", dir.as_ref().display()))?;
    Ok(result)
}

pub async fn temp_wit_file(package: &PackageRef, bytes: &[u8]) -> anyhow::Result<NamedTempFile> {
    // Sanitize the package ref for use as a filename prefix: `namespace:name`
    // contains characters (`:`, `/`) that are invalid in filenames on some
    // platforms (notably Windows).
    let prefix: String = package.to_string().replace([':', '/'], "_");
    let tmp_handle = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(".wasm")
        .tempfile()
        .context("Failed to create temporary file for built WIT package")
        .with_context(|| format!("package: {package}"))?;
    tokio::fs::write(tmp_handle.path(), &bytes)
        .await
        .context("Failed to write built WIT package to temp file")
        .with_context(|| format!("package: {package}"))?;

    tracing::debug!(tmp_pkg_path = %tmp_handle.path().display(), "Wrote temporary WIT package file");
    Ok(tmp_handle)
}

impl FetchArgs {
    pub async fn run(self) -> anyhow::Result<()> {
        let cwd = std::env::current_dir()?;
        let root = Manifest::load_root_workspace(&cwd).await?;

        let dirs = if let Some(dir) = self.dir.clone() {
            vec![dir]
        } else {
            root.as_ref()
                .map(|r| r.members.clone())
                .unwrap_or_else(|| vec![PathBuf::from("wit")])
        };
        let manifest = match root.as_ref() {
            Some(root) => {
                let manifest_path = root.root_dir().join(MANIFEST_FILE_NAME);
                Manifest::load_from_path(manifest_path).await?
            }
            None => Manifest::load().await?,
        };
        let output = self.output_type.unwrap_or_default();

        for dir in &dirs {
            check_dir(dir).await?;
        }

        match root {
            Some(root) => {
                self.run_workspace_fetch(&dirs, output, &manifest, root)
                    .await
            }
            None => self.fetch_into_lock(&dirs, &manifest, output).await,
        }
    }

    // fetch dependneces for a given workspace root, merging dependencies trees for included packages
    async fn run_workspace_fetch(
        &self,
        dirs: &[PathBuf],
        output: OutputType,
        manifest: &Manifest,
        root: WorkspaceRootConfig,
    ) -> anyhow::Result<()> {
        // Load/create the root lock file. This is the file that will be committed back to disk
        let lock_path = root.root_dir().join(LOCK_FILE_NAME);
        let mut lock_file = load_or_create_lock(&lock_path).await?;

        let verifier = PublishVerifier::try_new(
            root.members.as_ref(),
            "tmp_local_fetch",
            self.common.load_config().await?,
            self.common.load_cache().await?,
            &mut lock_file,
            false,
        )
        .await?;

        // Resolve dependencies for every requested member through the publish verifier
        let mut merged = DependencyResolutionMap::default();
        for dir in dirs {
            let resolved =
                wit::resolve_dependencies(manifest, dir, Some(&lock_file), verifier.client.clone())
                    .await
                    .with_context(|| {
                        format!("failed to resolve dependencies for {}", dir.display())
                    })?;
            for (pkg, resolution) in resolved.as_ref() {
                if verifier.packages.contains(pkg) {
                    continue;
                }
                merged.insert(pkg.clone(), resolution.clone());
            }
        }

        lock_file.update_dependencies(&merged);
        lock_file
            .write()
            .await
            .with_context(|| format!("failed to commit lock file at {}", lock_path.display()))?;

        // Ensure `<root-dir>/wkg/` exists and drop the aggregated deps into `<root-dir>/wkg/deps`.
        // `populate_dependencies` canonicalizes its argument, so the parent must exist.
        let out_dir = root.out_dir();
        tokio::fs::create_dir_all(&out_dir)
            .await
            .with_context(|| format!("failed to create {}", out_dir.display()))?;
        wit::populate_dependencies_workspace(&out_dir, &merged, output)
            .await
            .with_context(|| {
                format!(
                    "failed to populate workspace deps at {}",
                    out_dir.join(WIT_DEPS_DIR).display(),
                )
            })
    }

    /// Iterate `dirs` and run [`wit::fetch_dependencies`] unioning each call's resolved lock entries into a
    /// single set.
    /// `fetch_dependencies` replaces [`LockFile`] packages on every call so we snapshot
    /// between calls to avoid losing earlier entries.
    async fn fetch_into_lock(
        &self,
        dirs: &[PathBuf],
        manifest: &Manifest,
        output: OutputType,
    ) -> anyhow::Result<()> {
        let client = self.common.get_client().await?;
        let mut lock_file = LockFile::load(false).await?;

        let mut union: BTreeSet<LockedPackage> = BTreeSet::new();
        merge_locked_packages(&mut union, std::mem::take(&mut lock_file.packages));
        for dir in dirs {
            wit::fetch_dependencies(manifest, dir, &mut lock_file, client.clone(), output)
                .await
                .with_context(|| format!("failed to fetch dependencies for {}", dir.display()))?;
            merge_locked_packages(&mut union, std::mem::take(&mut lock_file.packages));
        }
        lock_file.packages = union;

        lock_file.write().await?;
        Ok(())
    }
}

/// Open `<root-dir>/wkg.lock` for read-write, creating it as an empty lockfile if missing.
/// Mirrors `LockFile::load`'s create-if-absent semantics but at an explicit path.
async fn load_or_create_lock(path: &Path) -> anyhow::Result<LockFile> {
    if !tokio::fs::try_exists(path).await? {
        let mut empty = LockFile::new_with_path([], path).await?;
        empty.write().await?;
        drop(empty);
    }
    LockFile::load_from_path(path, false)
        .await
        .with_context(|| format!("failed to load lock file at {}", path.display()))
}

/// Merge `incoming` locked packages into `acc`, unioning per-package version
/// entries when the same `(name, registry)` key appears in both sets. Newer
/// version entries win on conflicting requirements within a package.
fn merge_locked_packages(acc: &mut BTreeSet<LockedPackage>, incoming: BTreeSet<LockedPackage>) {
    for pkg in incoming {
        if let Some(existing) = acc.take(&pkg) {
            let mut merged = existing;
            for v in pkg.versions {
                if let Some(slot) = merged
                    .versions
                    .iter_mut()
                    .find(|e| e.requirement == v.requirement)
                {
                    *slot = v;
                } else {
                    merged.versions.push(v);
                }
            }
            acc.insert(merged);
        } else {
            acc.insert(pkg);
        }
    }
}

impl UpdateArgs {
    pub async fn run(self) -> anyhow::Result<()> {
        check_dir(&self.dir).await?;
        let client = self.common.get_client().await?;
        let manifest = Manifest::load().await?;
        let mut lock_file = LockFile::load(false).await?;
        // Clear the lock file since we're updating it
        lock_file.packages.clear();
        wit::fetch_dependencies(
            &manifest,
            self.dir,
            &mut lock_file,
            client,
            self.output_type.unwrap_or_default(),
        )
        .await?;
        // Now write out the lock file since everything else succeeded
        lock_file.write().await?;
        todo!()
    }
}

async fn check_dir(dir: impl AsRef<Path>) -> anyhow::Result<()> {
    let dir = dir.as_ref();
    tokio::fs::metadata(dir).await
        .with_context(|| format!("unable to read wit directory: {}", dir.display()))
        .context("This command should be run from the parent directory of the wit directory or a directory can be overridden with the --wit-dir argument")
        .map(|_|())
}
