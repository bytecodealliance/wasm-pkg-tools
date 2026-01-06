//! Functions for building WIT packages and fetching their dependencies.

use std::{collections::HashSet, path::Path, str::FromStr};

use anyhow::{Context, Result};
use semver::{Version, VersionReq};
use wasm_metadata::{AddMetadata, AddMetadataField};
use wasm_pkg_client::{
    caching::{CachingClient, FileCache},
    PackageRef,
};
use wit_component::WitPrinter;
use wit_parser::{PackageId, PackageName, Resolve};

use crate::{
    config::Config,
    lock::LockFile,
    resolver::{
        DecodedDependency, Dependency, DependencyResolution, DependencyResolutionMap,
        DependencyResolver, LocalResolution, RegistryPackage,
    },
};

/// The supported output types for WIT deps
#[derive(Debug, Clone, Copy, Default)]
pub enum OutputType {
    /// Output each dependency as a WIT file in the deps directory.
    #[default]
    Wit,
    /// Output each dependency as a wasm binary file in the deps directory.
    Wasm,
}

impl FromStr for OutputType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let lower_trim = s.trim().to_lowercase();
        match lower_trim.as_str() {
            "wit" => Ok(Self::Wit),
            "wasm" => Ok(Self::Wasm),
            _ => Err(anyhow::anyhow!("Invalid output type: {}", s)),
        }
    }
}

/// Builds a WIT package given the configuration and directory to parse. Will update the given lock
/// file with the resolved dependencies but will not write it to disk.
pub async fn build_package(
    config: &Config,
    wit_dir: impl AsRef<Path>,
    lock_file: &mut LockFile,
    client: CachingClient<FileCache>,
) -> Result<(PackageRef, Option<Version>, Vec<u8>)> {
    let dependencies = resolve_dependencies(config, &wit_dir, Some(lock_file), client)
        .await
        .context("Unable to resolve dependencies")?;

    lock_file.update_dependencies(&dependencies);

    let (resolve, pkg_id) = dependencies.generate_resolve(wit_dir).await?;
    let bytes = wit_component::encode(&resolve, pkg_id)?;

    let pkg = &resolve.packages[pkg_id];
    let name = PackageRef::new(
        pkg.name
            .namespace
            .parse()
            .context("Invalid namespace found in package")?,
        pkg.name
            .name
            .parse()
            .context("Invalid name found in package")?,
    );
    let version = pkg
        .name
        .version
        .as_ref()
        .map(|v| v.to_string().parse())
        .transpose()
        .context("Invalid version found in package")?;

    let processed_by_version = option_env!("WIT_VERSION_INFO").unwrap_or(env!("CARGO_PKG_VERSION"));

    let metadata = config.metadata.clone().unwrap_or_default();
    let add_metadata = {
        /// MetadataField::Set iff the given Option is Some
        fn set<T: std::fmt::Debug + Clone>(opt: Option<T>) -> AddMetadataField<T> {
            opt.map(AddMetadataField::Set).unwrap_or_default()
        }
        let mut add = AddMetadata::default();
        add.name = set(Some(format!("{}:{}", pkg.name.namespace, pkg.name.name)));
        add.processed_by = vec![(
            env!("CARGO_PKG_NAME").to_string(),
            processed_by_version.to_string(),
        )];
        add.authors = set(metadata.authors.map(|v| v.parse()).transpose()?);
        add.description = set(metadata.description.map(|v| v.parse()).transpose()?);
        add.licenses = set(metadata.licenses.map(|v| v.parse()).transpose()?);
        add.source = set(metadata.source.map(|v| v.parse()).transpose()?);
        add.homepage = set(metadata.homepage.map(|v| v.parse()).transpose()?);
        add.revision = set(metadata.revision.map(|v| v.parse()).transpose()?);
        add.version = set(version);
        add
    };
    let bytes = add_metadata.to_wasm(&bytes)?;

    Ok((name, pkg.name.version.clone(), bytes))
}

/// Fetches and optionally updates all dependencies for the given path and writes them in the
/// specified format. The lock file will be updated with the resolved dependencies but will not be
/// written to disk.
///
/// This is mostly a convenience wrapper around [`resolve_dependencies`] and [`populate_dependencies`].
pub async fn fetch_dependencies(
    config: &Config,
    wit_dir: impl AsRef<Path>,
    lock_file: &mut LockFile,
    client: CachingClient<FileCache>,
    output: OutputType,
) -> Result<()> {
    // Don't pass lock file if update is true
    let dependencies = resolve_dependencies(config, &wit_dir, Some(lock_file), client).await?;
    lock_file.update_dependencies(&dependencies);
    populate_dependencies(wit_dir, &dependencies, output).await
}

/// Generate the list of all packages and their version requirement from the given path (a directory
/// or file).
///
/// This is a lower level function exposed for convenience that is used by higher level functions
/// for resolving dependencies.
pub fn get_packages(
    path: impl AsRef<Path>,
) -> Result<(PackageRef, HashSet<(PackageRef, VersionReq)>)> {
    let group =
        wit_parser::UnresolvedPackageGroup::parse_path(path).context("Couldn't parse package")?;

    let name = PackageRef::new(
        group
            .main
            .name
            .namespace
            .parse()
            .context("Invalid namespace found in package")?,
        group
            .main
            .name
            .name
            .parse()
            .context("Invalid name found in package")?,
    );

    // Get all package refs from the main package and then from any nested packages
    let packages: HashSet<(PackageRef, VersionReq)> =
        packages_from_foreign_deps(group.main.foreign_deps.into_keys())
            .chain(
                group
                    .nested
                    .into_iter()
                    .flat_map(|pkg| packages_from_foreign_deps(pkg.foreign_deps.into_keys())),
            )
            .collect();

    Ok((name, packages))
}

/// Builds a list of resolved dependencies loaded from the component or path containing the WIT.
/// This will configure the resolver, override any dependencies from configuration and resolve the
/// dependency map. This map can then be used in various other functions for fetching the
/// dependencies and/or building a final resolved package.
pub async fn resolve_dependencies(
    config: &Config,
    path: impl AsRef<Path>,
    lock_file: Option<&LockFile>,
    client: CachingClient<FileCache>,
) -> Result<DependencyResolutionMap> {
    let mut resolver = DependencyResolver::new_with_client(client, lock_file)?;
    // add deps from config first in case they're local deps and then add deps from the directory
    if let Some(overrides) = config.overrides.as_ref() {
        for (pkg, ovr) in overrides.iter() {
            let pkg: PackageRef = pkg.parse().context("Unable to parse as a package ref")?;
            let dep = match (ovr.path.as_ref(), ovr.version.as_ref()) {
                (Some(path), v) => {
                    if v.is_some() {
                        tracing::warn!("Ignoring version override for local package");
                    }
                    let path = tokio::fs::canonicalize(path)
                        .await
                        .with_context(|| format!("{}", path.display()))?;
                    Dependency::Local(path)
                }
                (None, Some(version)) => Dependency::Package(RegistryPackage {
                    name: Some(pkg.clone()),
                    version: version.to_owned(),
                    registry: None,
                }),
                (None, None) => {
                    tracing::warn!("Found override without version or path, ignoring");
                    continue;
                }
            };

            tracing::debug!(dependency = %dep);
            resolver
                .add_dependency(&pkg, &dep)
                .await
                .with_context(|| dep.clone())
                .context("Unable to add dependency")?;
        }
    }
    let (_name, packages) = get_packages(path)?;
    resolver.add_packages(packages).await?;
    resolver.resolve().await
}

/// Populate a list of dependencies into the given directory. If the directory does not exist it
/// will be created. Any existing files in the directory will be deleted. The dependencies will be
/// put into the `deps` subdirectory within the directory in the format specified by the output
/// type. Please note that if a local dep is encountered when using [`OutputType::Wasm`] and it
/// isn't a wasm binary, it will be copied directly to the directory and not packaged into a wit
/// package first
pub async fn populate_dependencies(
    path: impl AsRef<Path>,
    deps: &DependencyResolutionMap,
    output: OutputType,
) -> Result<()> {
    // Canonicalizing will error if the path doesn't exist, so we don't need to check for that
    let path = tokio::fs::canonicalize(path).await?;
    let metadata = tokio::fs::metadata(&path).await?;
    if !metadata.is_dir() {
        anyhow::bail!("Path is not a directory");
    }
    let deps_path = path.join("deps");
    // Remove the whole directory if it already exists and then recreate
    if let Err(e) = tokio::fs::remove_dir_all(&deps_path).await {
        // If the directory doesn't exist, ignore the error
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(anyhow::anyhow!("Unable to remove deps directory: {e}"));
        }
    }
    tokio::fs::create_dir_all(&deps_path).await?;

    // For wit output, generate the resolve and then output each package in the resolve
    if let OutputType::Wit = output {
        let (resolve, pkg_id) = deps.generate_resolve(&path).await?;
        return print_wit_from_resolve(&resolve, pkg_id, &deps_path).await;
    }

    // If we got binary output, write them instead of the wit
    let decoded_deps = deps.decode_dependencies().await?;

    for (name, dep) in decoded_deps.iter() {
        let mut output_path = deps_path.join(name_from_package_name(name));

        match dep {
            DecodedDependency::Wit {
                resolution: DependencyResolution::Local(local),
                ..
            } => {
                // Local deps always need to be written to a subdirectory of deps so create that here
                tokio::fs::create_dir_all(&output_path).await?;
                write_local_dep(local, output_path).await?;
            }
            // This case shouldn't happen because registries only support wit packages. We can't get
            // a resolve from the unresolved group, so error out here. Ideally we could print the
            // unresolved group, but WitPrinter doesn't support that yet
            DecodedDependency::Wit {
                resolution: DependencyResolution::Registry(_),
                ..
            } => {
                anyhow::bail!("Unable to resolve dependency, this is a programmer error");
            }
            // Right now WIT packages include all of their dependencies, so we don't need to fetch
            // those too. In the future, we'll need to look for unsatisfied dependencies and fetch
            // them
            DecodedDependency::Wasm { resolution, .. } => {
                // This is going to be written to a single file, so we don't create a directory here
                // NOTE(thomastaylor312): This janky looking thing is to avoid chopping off the
                // patch number from the release. Once `add_extension` is stabilized, we can use
                // that instead
                let mut file_name = output_path.file_name().unwrap().to_owned();
                file_name.push(".wasm");
                output_path.set_file_name(file_name);
                match resolution {
                    DependencyResolution::Local(local) => {
                        let meta = tokio::fs::metadata(&local.path).await?;
                        if !meta.is_file() {
                            anyhow::bail!("Local dependency is not single wit package file");
                        }
                        tokio::fs::copy(&local.path, output_path)
                            .await
                            .context("Unable to copy local dependency")?;
                    }
                    DependencyResolution::Registry(registry) => {
                        let mut reader = registry.fetch().await?;
                        let mut output_file = tokio::fs::File::create(output_path).await?;
                        tokio::io::copy(&mut reader, &mut output_file).await?;
                        output_file.sync_all().await?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn packages_from_foreign_deps(
    deps: impl IntoIterator<Item = PackageName>,
) -> impl Iterator<Item = (PackageRef, VersionReq)> {
    deps.into_iter().filter_map(|dep| {
        let name = PackageRef::new(dep.namespace.parse().ok()?, dep.name.parse().ok()?);
        let version = match dep.version {
            Some(v) => format!("={v}"),
            None => "*".to_string(),
        };
        Some((
            name,
            version
                .parse()
                .expect("Unable to parse into version request, this is programmer error"),
        ))
    })
}

async fn write_local_dep(local: &LocalResolution, output_path: impl AsRef<Path>) -> Result<()> {
    let meta = tokio::fs::metadata(&local.path).await?;
    if meta.is_file() {
        tokio::fs::copy(
            &local.path,
            output_path.as_ref().join(local.path.file_name().unwrap()),
        )
        .await?;
    } else {
        // For now, don't try to recurse, since most of the tools don't recurse unless
        // there is a deps folder anyway, which we don't care about here
        let mut dir = tokio::fs::read_dir(&local.path).await?;
        while let Some(entry) = dir.next_entry().await? {
            if !entry.metadata().await?.is_file() {
                continue;
            }
            let entry_path = entry.path();
            tokio::fs::copy(
                &entry_path,
                output_path.as_ref().join(entry_path.file_name().unwrap()),
            )
            .await?;
        }
    }
    Ok(())
}

async fn print_wit_from_resolve(
    resolve: &Resolve,
    top_level_id: PackageId,
    root_deps_dir: &Path,
) -> Result<()> {
    for (id, pkg) in resolve
        .packages
        .iter()
        .filter(|(id, _)| *id != top_level_id)
    {
        let dep_path = root_deps_dir.join(name_from_package_name(&pkg.name));
        tokio::fs::create_dir_all(&dep_path).await?;
        let mut printer = WitPrinter::default();
        printer
            .print(resolve, id, &[])
            .context("Unable to print wit")?;
        tokio::fs::write(dep_path.join("package.wit"), &printer.output.to_string()).await?;
    }
    Ok(())
}

/// Given a package name, returns a valid directory/file name for it (thanks windows!)
fn name_from_package_name(package_name: &PackageName) -> String {
    let package_name_str = package_name.to_string();
    package_name_str.replace([':', '@'], "-")
}
