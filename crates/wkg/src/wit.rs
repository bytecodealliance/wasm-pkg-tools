//! Args and commands for interacting with WIT files and dependencies
use std::path::{Path, PathBuf};

use anstream::eprintln;
use anyhow::Context;
use clap::{Args, Subcommand};
use tempfile::NamedTempFile;
use wasm_pkg_client::caching::{CachingClient, FileCache};
use wasm_pkg_common::package::{PackageRef, Version};
use wasm_pkg_core::{
    lock::LockFile,
    manifest::Manifest,
    wit::{self, OutputType},
};

use crate::Common;

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
    /// current directory with the name of the package
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
    #[clap(short = 'd', long = "wit-dir", default_value = "wit")]
    pub dir: PathBuf,

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
        check_dir(&self.dir).await?;
        let client = self.common.get_client().await?;
        let manifest = Manifest::load().await?;
        let mut lock_file = LockFile::load(false).await?;
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
        Ok(())
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
