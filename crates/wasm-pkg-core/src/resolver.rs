//! A resolver for resolving dependencies from a component registry.
// NOTE(thomastaylor312): This is copied and adapted from the `cargo-component` crate: https://github.com/bytecodealliance/cargo-component/blob/f0be1c7d9917aa97e9102e69e3b838dae38d624b/crates/core/src/registry.rs

use std::{
    collections::{hash_map, HashMap, HashSet},
    fmt::Debug,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{bail, Context, Result};
use futures_util::TryStreamExt;
use indexmap::{IndexMap, IndexSet};
use semver::{Comparator, Op, Version, VersionReq};
use tokio::io::{AsyncRead, AsyncReadExt};
use wasm_pkg_client::{
    caching::{CachingClient, FileCache},
    Client, Config, ContentDigest, Error as WasmPkgError, PackageRef, Release, VersionInfo,
};
use wit_component::DecodedWasm;
use wit_parser::{PackageId, PackageName, Resolve, UnresolvedPackageGroup, WorldId};

use crate::{lock::LockFile, wit::get_packages};

/// The name of the default registry.
pub const DEFAULT_REGISTRY_NAME: &str = "default";

// TODO: functions for resolving dependencies from a lock file

/// Represents a WIT package dependency.
#[derive(Debug, Clone)]
pub enum Dependency {
    /// The dependency is a registry package.
    Package(RegistryPackage),

    /// The dependency is a path to a local directory or file.
    Local(PathBuf),
}

impl std::fmt::Display for Dependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Dependency::Package(RegistryPackage {
                name,
                version,
                registry,
            }) => {
                let registry = registry.as_deref().unwrap_or("_");
                let name = name.as_ref().map(|n| n.to_string());

                write!(
                    f,
                    "{{ registry =  {registry} package = {}@{version} }}",
                    name.as_deref().unwrap_or("_:_"),
                )
            }
            Dependency::Local(path_buf) => write!(f, "{}", path_buf.display()),
        }
    }
}

impl FromStr for Dependency {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Ok(Self::Package(s.parse()?))
    }
}

/// Represents a reference to a registry package.
#[derive(Debug, Clone)]
pub struct RegistryPackage {
    /// The name of the package.
    ///
    /// If not specified, the name from the mapping will be used.
    pub name: Option<PackageRef>,

    /// The version requirement of the package.
    pub version: VersionReq,

    /// The name of the component registry containing the package.
    ///
    /// If not specified, the default registry is used.
    pub registry: Option<String>,
}

impl FromStr for RegistryPackage {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Ok(Self {
            name: None,
            version: s
                .parse()
                .with_context(|| format!("'{s}' is an invalid registry package version"))?,
            registry: None,
        })
    }
}

/// Represents information about a resolution of a registry package.
#[derive(Clone)]
pub struct RegistryResolution {
    /// The name of the dependency that was resolved.
    ///
    /// This may differ from `package` if the dependency was renamed.
    pub name: PackageRef,
    /// The name of the package from the registry that was resolved.
    pub package: PackageRef,
    /// The name of the registry used to resolve the package if one was provided
    pub registry: Option<String>,
    /// The version requirement that was used to resolve the package.
    pub requirement: VersionReq,
    /// The package version that was resolved.
    pub version: Version,
    /// The digest of the package contents.
    pub digest: ContentDigest,
    /// The client to use for fetching the package contents.
    client: CachingClient<FileCache>,
}

impl Debug for RegistryResolution {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("RegistryResolution")
            .field("name", &self.name)
            .field("package", &self.package)
            .field("registry", &self.registry)
            .field("requirement", &self.requirement)
            .field("version", &self.version)
            .field("digest", &self.digest)
            .finish()
    }
}

impl RegistryResolution {
    /// Fetches the raw package bytes from the registry. Returns an AsyncRead that will stream the
    /// package contents
    pub async fn fetch(&self) -> Result<impl AsyncRead> {
        let stream = self
            .client
            .get_content(
                &self.package,
                &Release {
                    version: self.version.clone(),
                    content_digest: self.digest.clone(),
                },
            )
            .await?;

        Ok(tokio_util::io::StreamReader::new(
            stream.map_err(std::io::Error::other),
        ))
    }
}

/// Represents information about a resolution of a local file.
#[derive(Clone, Debug)]
pub struct LocalResolution {
    /// The name of the dependency that was resolved.
    pub name: PackageRef,
    /// The path to the resolved dependency.
    pub path: PathBuf,
}

/// Represents a resolution of a dependency.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum DependencyResolution {
    /// The dependency is resolved from a registry package.
    Registry(RegistryResolution),
    /// The dependency is resolved from a local path.
    Local(LocalResolution),
}

impl DependencyResolution {
    /// Gets the name of the dependency that was resolved.
    pub fn name(&self) -> &PackageRef {
        match self {
            Self::Registry(res) => &res.name,
            Self::Local(res) => &res.name,
        }
    }

    /// Gets the resolved version.
    ///
    /// Returns `None` if the dependency is not resolved from a registry package.
    pub fn version(&self) -> Option<&Version> {
        match self {
            Self::Registry(res) => Some(&res.version),
            Self::Local(_) => None,
        }
    }

    /// The key used in sorting and searching the lock file package list.
    ///
    /// Returns `None` if the dependency is not resolved from a registry package.
    pub fn key(&self) -> Option<(&PackageRef, Option<&str>)> {
        match self {
            DependencyResolution::Registry(pkg) => Some((&pkg.package, pkg.registry.as_deref())),
            DependencyResolution::Local(_) => None,
        }
    }

    /// Decodes the resolved dependency.
    pub async fn decode(&self) -> Result<DecodedDependency<'_>> {
        // If the dependency path is a directory, assume it contains wit to parse as a package.
        let bytes = match self {
            DependencyResolution::Local(LocalResolution { path, .. })
                if tokio::fs::metadata(path).await?.is_dir() =>
            {
                return Ok(DecodedDependency::Wit {
                    resolution: self,
                    package: UnresolvedPackageGroup::parse_dir(path).with_context(|| {
                        format!("failed to parse dependency `{path}`", path = path.display())
                    })?,
                });
            }
            DependencyResolution::Local(LocalResolution { path, .. }) => {
                tokio::fs::read(path).await.with_context(|| {
                    format!(
                        "failed to read content of dependency `{name}` at path `{path}`",
                        name = self.name(),
                        path = path.display()
                    )
                })?
            }
            DependencyResolution::Registry(res) => {
                let mut reader = res.fetch().await?;

                let mut buf = Vec::new();
                reader.read_to_end(&mut buf).await?;
                buf
            }
        };

        if &bytes[0..4] != b"\0asm" {
            return Ok(DecodedDependency::Wit {
                resolution: self,
                package: UnresolvedPackageGroup::parse(
                    // This is fake, but it's needed for the parser to work.
                    self.name().to_string(),
                    std::str::from_utf8(&bytes).with_context(|| {
                        format!(
                            "dependency `{name}` is not UTF-8 encoded",
                            name = self.name()
                        )
                    })?,
                )?,
            });
        }

        Ok(DecodedDependency::Wasm {
            resolution: self,
            decoded: wit_component::decode(&bytes).with_context(|| {
                format!(
                    "failed to decode content of dependency `{name}`",
                    name = self.name(),
                )
            })?,
        })
    }
}

/// Represents a decoded dependency.
pub enum DecodedDependency<'a> {
    /// The dependency decoded from an unresolved WIT package.
    Wit {
        /// The resolution related to the decoded dependency.
        resolution: &'a DependencyResolution,
        /// The unresolved WIT package.
        package: UnresolvedPackageGroup,
    },
    /// The dependency decoded from a Wasm file.
    Wasm {
        /// The resolution related to the decoded dependency.
        resolution: &'a DependencyResolution,
        /// The decoded Wasm file.
        decoded: DecodedWasm,
    },
}

impl DecodedDependency<'_> {
    /// Fully resolves the dependency.
    ///
    /// If the dependency is an unresolved WIT package, it will assume that the
    /// package has no foreign dependencies.
    pub fn resolve(self) -> Result<(Resolve, PackageId, Vec<PathBuf>)> {
        match self {
            Self::Wit { package, .. } => {
                let mut resolve = Resolve::new();
                resolve.all_features = true;
                let source_files = package
                    .source_map
                    .source_files()
                    .map(Path::to_path_buf)
                    .collect();
                let pkg = resolve.push_group(package)?;
                Ok((resolve, pkg, source_files))
            }
            Self::Wasm { decoded, .. } => match decoded {
                DecodedWasm::WitPackage(resolve, pkg) => Ok((resolve, pkg, Vec::new())),
                DecodedWasm::Component(resolve, world) => {
                    let pkg = resolve.worlds[world].package.unwrap();
                    Ok((resolve, pkg, Vec::new()))
                }
            },
        }
    }

    /// Gets the package name of the decoded dependency.
    pub fn package_name(&self) -> &PackageName {
        match self {
            Self::Wit { package, .. } => &package.main.name,
            Self::Wasm { decoded, .. } => &decoded.resolve().packages[decoded.package()].name,
        }
    }

    /// Converts the decoded dependency into a component world.
    ///
    /// Returns an error if the dependency is not a decoded component.
    pub fn into_component_world(self) -> Result<(Resolve, WorldId)> {
        match self {
            Self::Wasm {
                decoded: DecodedWasm::Component(resolve, world),
                ..
            } => Ok((resolve, world)),
            _ => bail!("dependency is not a WebAssembly component"),
        }
    }
}

/// Used to resolve dependencies for a WIT package.
pub struct DependencyResolver<'a> {
    client: CachingClient<FileCache>,
    lock_file: Option<&'a LockFile>,
    packages: HashMap<PackageRef, Vec<VersionInfo>>,
    dependencies: HashMap<PackageRef, RegistryDependency>,
    resolutions: DependencyResolutionMap,
}

impl<'a> DependencyResolver<'a> {
    /// Creates a new dependency resolver. If `config` is `None`, then the resolver will be set to
    /// offline mode and a lock file must be given as well. Anything that will require network
    /// access will fail in offline mode.
    pub fn new(
        config: Option<Config>,
        lock_file: Option<&'a LockFile>,
        cache: FileCache,
    ) -> anyhow::Result<Self> {
        if config.is_none() && lock_file.is_none() {
            anyhow::bail!("lock file must be provided when offline mode is enabled");
        }
        let client = CachingClient::new(config.map(Client::new), cache);
        Ok(DependencyResolver {
            client,
            lock_file,
            resolutions: Default::default(),
            packages: Default::default(),
            dependencies: Default::default(),
        })
    }

    /// Creates a new dependency resolver with the given client. This is useful when you already
    /// have a client available. If the client is set to offline mode, then a lock file must be
    /// given or this will error
    pub fn new_with_client(
        client: CachingClient<FileCache>,
        lock_file: Option<&'a LockFile>,
    ) -> anyhow::Result<Self> {
        if client.is_readonly() && lock_file.is_none() {
            anyhow::bail!("lock file must be provided when offline mode is enabled");
        }
        Ok(DependencyResolver {
            client,
            lock_file,
            resolutions: Default::default(),
            packages: Default::default(),
            dependencies: Default::default(),
        })
    }

    /// Add a dependency to the resolver. If the dependency already exists, then it will be ignored.
    /// To override an existing dependency, use [`override_dependency`](Self::override_dependency).
    pub async fn add_dependency(
        &mut self,
        name: &PackageRef,
        dependency: &Dependency,
    ) -> Result<()> {
        self.add_dependency_internal(name, dependency, false).await
    }

    /// Add a dependency to the resolver. If the dependency already exists, then it will be
    /// overridden.
    pub async fn override_dependency(
        &mut self,
        name: &PackageRef,
        dependency: &Dependency,
    ) -> Result<()> {
        self.add_dependency_internal(name, dependency, true).await
    }

    async fn add_dependency_internal(
        &mut self,
        name: &PackageRef,
        dependency: &Dependency,
        force_override: bool,
    ) -> Result<()> {
        match dependency {
            Dependency::Package(package) => {
                // Dependency comes from a registry, add a dependency to the resolver
                let registry_name = package.registry.as_deref().or_else(|| {
                    self.client.client().ok().and_then(|client| {
                        client
                            .config()
                            .resolve_registry(name)
                            .map(|reg| reg.as_ref())
                    })
                });
                let package_name = package.name.clone().unwrap_or_else(|| name.clone());

                // Resolve the version from the lock file if there is one
                let locked = match self.lock_file.as_ref().and_then(|resolver| {
                    resolver
                        .resolve(registry_name, &package_name, &package.version)
                        .transpose()
                }) {
                    Some(Ok(locked)) => Some(locked),
                    Some(Err(e)) => return Err(e),
                    _ => None,
                };

                // So if it wasn't already fetched first? then we'll try and resolve it later, and the override
                // is not present there for some reason
                if !force_override
                    && (self.resolutions.contains_key(name) || self.dependencies.contains_key(name))
                {
                    tracing::debug!(%name, "dependency already exists and override is not set, ignoring");
                    return Ok(());
                }
                self.dependencies.insert(
                    name.to_owned(),
                    RegistryDependency {
                        package: package_name,
                        version: package.version.clone(),
                        locked: locked.map(|l| (l.version.clone(), l.digest.clone())),
                    },
                );
            }
            Dependency::Local(p) => {
                let res = DependencyResolution::Local(LocalResolution {
                    name: name.clone(),
                    path: p.clone(),
                });

                // This is a bit of a hack, but if there are multiple local dependencies that are
                // nested and overridden, getting the packages from the local package treats _all_
                // deps as registry deps. So if we're handling a local path and the dependencies
                // have a registry package already, override it. Otherwise follow normal overrides.
                // We should definitely fix this and change where we resolve these things
                let should_insert = force_override
                    || self.dependencies.contains_key(name)
                    || !self.resolutions.contains_key(name);
                if !should_insert {
                    tracing::debug!(%name, "dependency already exists and registry override is not set, ignoring");
                    return Ok(());
                }

                // Because we got here, we should remove anything from dependencies that is the same
                // package because we're overriding with the local package. Technically we could be
                // clever and just do this in the boolean above, but I'm paranoid
                self.dependencies.remove(name);

                // Now that we check we haven't already inserted this dep, get the packages from the
                // local dependency and add those to the resolver before adding the dependency
                let (_, packages) = get_packages(p)
                    .context("Error getting dependent packages from local dependency")?;
                Box::pin(self.add_packages(packages))
                    .await
                    .context("Error adding packages to resolver for local dependency")?;

                let prev = self.resolutions.insert(name.clone(), res);
                assert!(prev.is_none());
            }
        }

        Ok(())
    }

    /// A helper function for adding an iterator of package refs and their associated version
    /// requirements to the resolver
    pub async fn add_packages(
        &mut self,
        packages: impl IntoIterator<Item = (PackageRef, VersionReq)>,
    ) -> Result<()> {
        for (package, req) in packages {
            self.add_dependency(
                &package,
                &Dependency::Package(RegistryPackage {
                    name: Some(package.clone()),
                    version: req,
                    registry: None,
                }),
            )
            .await?;
        }
        Ok(())
    }

    /// Resolve all dependencies.
    ///
    /// This will download all dependencies that are not already present in client storage.
    ///
    /// Returns the dependency resolution map.
    pub async fn resolve(mut self) -> Result<DependencyResolutionMap> {
        let mut resolutions = self.resolutions;
        for (name, dependency) in self.dependencies.into_iter() {
            // We need to clone a handle to the client because we mutably borrow self below. Might
            // be worth replacing the mutable borrow with a RwLock down the line.
            let client = self.client.clone();

            let (selected_version, digest) = if client.is_readonly() {
                dependency
                    .locked
                    .as_ref()
                    .map(|(ver, digest)| (ver, Some(digest)))
                    .ok_or_else(|| {
                        anyhow::anyhow!("Couldn't find locked dependency while in offline mode")
                    })?
            } else {
                let versions =
                    load_package(&mut self.packages, &self.client, dependency.package.clone())
                        .await?
                        .with_context(|| {
                            format!(
                                "package `{name}` was not found in component registry",
                                name = dependency.package
                            )
                        })?;

                match &dependency.locked {
                    Some((version, digest)) => {
                        // The dependency had a lock file entry, so attempt to do an exact match first
                        let exact_req = VersionReq {
                            comparators: vec![Comparator {
                                op: Op::Exact,
                                major: version.major,
                                minor: Some(version.minor),
                                patch: Some(version.patch),
                                pre: version.pre.clone(),
                            }],
                        };

                        // If an exact match can't be found, fallback to the latest release to satisfy
                        // the version requirement; this can happen when packages are yanked. If we did
                        // find an exact match, return the digest for comparison after fetching the
                        // release
                        find_latest_release(versions, &exact_req).map(|v| (&v.version, Some(digest))).or_else(|| find_latest_release(versions, &dependency.version).map(|v| (&v.version, None)))
                    }
                    None => find_latest_release(versions, &dependency.version).map(|v| (&v.version, None)),
                }.with_context(|| format!("component registry package `{name}` has no release matching version requirement `{version}`", name = dependency.package, version = dependency.version))?
            };

            // We need to clone a handle to the client because we mutably borrow self above. Might
            // be worth replacing the mutable borrow with a RwLock down the line.
            let release = client
                .get_release(&dependency.package, selected_version)
                .await?;
            if let Some(digest) = digest {
                if &release.content_digest != digest {
                    bail!(
                        "component registry package `{name}` (v`{version}`) has digest `{content}` but the lock file specifies digest `{digest}`",
                        name = dependency.package,
                        version = release.version,
                        content = release.content_digest,
                    );
                }
            }
            let resolution = RegistryResolution {
                name: name.clone(),
                package: dependency.package.clone(),
                registry: self.client.client().ok().and_then(|client| {
                    client
                        .config()
                        .resolve_registry(&name)
                        .map(ToString::to_string)
                }),
                requirement: dependency.version.clone(),
                version: release.version.clone(),
                digest: release.content_digest.clone(),
                client: self.client.clone(),
            };
            resolutions.insert(name, DependencyResolution::Registry(resolution));
        }

        Ok(resolutions)
    }
}

async fn load_package<'b>(
    packages: &'b mut HashMap<PackageRef, Vec<VersionInfo>>,
    client: &CachingClient<FileCache>,
    package: PackageRef,
) -> Result<Option<&'b Vec<VersionInfo>>> {
    match packages.entry(package) {
        hash_map::Entry::Occupied(e) => Ok(Some(e.into_mut())),
        hash_map::Entry::Vacant(e) => match client.list_all_versions(e.key()).await {
            Ok(p) => Ok(Some(e.insert(p))),
            Err(WasmPkgError::PackageNotFound) => Ok(None),
            Err(err) => Err(err.into()),
        },
    }
}

#[derive(Debug)]
struct RegistryDependency {
    /// The canonical package name of the registry package. In most cases, this is the same as the
    /// name but could be different if the given package name has been remapped
    package: PackageRef,
    version: VersionReq,
    locked: Option<(Version, ContentDigest)>,
}

fn find_latest_release<'a>(
    versions: &'a [VersionInfo],
    req: &VersionReq,
) -> Option<&'a VersionInfo> {
    versions
        .iter()
        .filter(|info| !info.yanked && req.matches(&info.version))
        .max_by(|a, b| a.version.cmp(&b.version))
}

// NOTE(thomastaylor312): This is copied from the old wit package in the cargo-component and broken
// out for some reuse. I don't know enough about resolvers to know if there is an easier way to
// write this, so any future people seeing this should feel free to refactor it if they know a
// better way to do it.

/// Represents a map of dependency resolutions.
///
/// The key to the map is the package name of the dependency.
#[derive(Debug, Clone, Default)]
pub struct DependencyResolutionMap(HashMap<PackageRef, DependencyResolution>);

impl AsRef<HashMap<PackageRef, DependencyResolution>> for DependencyResolutionMap {
    fn as_ref(&self) -> &HashMap<PackageRef, DependencyResolution> {
        &self.0
    }
}

impl Deref for DependencyResolutionMap {
    type Target = HashMap<PackageRef, DependencyResolution>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for DependencyResolutionMap {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl DependencyResolutionMap {
    /// Fetch all dependencies and ensure there are no circular dependencies. Returns the decoded
    /// dependencies (sorted topologically), ready to use for output or adding to a [`Resolve`].
    pub async fn decode_dependencies(
        &self,
    ) -> Result<IndexMap<PackageName, DecodedDependency<'_>>> {
        // Start by decoding all of the dependencies
        let mut deps = IndexMap::new();
        for (name, resolution) in self.0.iter() {
            let decoded = resolution.decode().await?;
            if let Some(prev) = deps.insert(decoded.package_name().clone(), decoded) {
                anyhow::bail!(
                    "duplicate definitions of package `{prev}` found while decoding dependency `{name}`",
                    prev = prev.package_name()
                );
            }
        }

        // Do a topological sort of the dependencies
        let mut order = IndexSet::new();
        let mut visiting = HashSet::new();
        for dep in deps.values() {
            visit(dep, &deps, &mut order, &mut visiting)?;
        }

        assert!(visiting.is_empty());

        // Now that we have the right order, re-order the dependencies to match
        deps.sort_by(|name_a, _, name_b, _| {
            order.get_index_of(name_a).cmp(&order.get_index_of(name_b))
        });

        Ok(deps)
    }

    /// Given a path to a component or a directory containing wit, use the given dependencies to
    /// generate a [`Resolve`] for the root package.
    pub async fn generate_resolve(&self, dir: impl AsRef<Path>) -> Result<(Resolve, PackageId)> {
        let mut merged = Resolve {
            // Retain @unstable features; downstream tooling will process them further
            all_features: true,
            ..Resolve::default()
        };

        let deps = self.decode_dependencies().await?;

        // Parse the root package itself
        let root = UnresolvedPackageGroup::parse_dir(&dir).with_context(|| {
            format!(
                "failed to parse package from directory `{dir}`",
                dir = dir.as_ref().display()
            )
        })?;

        let mut source_files: Vec<_> = root
            .source_map
            .source_files()
            .map(Path::to_path_buf)
            .collect();

        // Merge all of the dependencies first
        for decoded in deps.into_values() {
            match decoded {
                DecodedDependency::Wit {
                    resolution,
                    package,
                } => {
                    source_files.extend(package.source_map.source_files().map(Path::to_path_buf));
                    merged.push_group(package).with_context(|| {
                        format!(
                            "failed to merge dependency `{name}`",
                            name = resolution.name()
                        )
                    })?;
                }
                DecodedDependency::Wasm {
                    resolution,
                    decoded,
                } => {
                    let resolve = match decoded {
                        DecodedWasm::WitPackage(resolve, _) => resolve,
                        DecodedWasm::Component(resolve, _) => resolve,
                    };

                    merged.merge(resolve).with_context(|| {
                        format!(
                            "failed to merge world of dependency `{name}`",
                            name = resolution.name()
                        )
                    })?;
                }
            };
        }

        let package = merged.push_group(root).with_context(|| {
            format!(
                "failed to merge package from directory `{dir}`",
                dir = dir.as_ref().display()
            )
        })?;

        Ok((merged, package))
    }
}

fn visit<'a>(
    dep: &'a DecodedDependency<'a>,
    deps: &'a IndexMap<PackageName, DecodedDependency>,
    order: &mut IndexSet<PackageName>,
    visiting: &mut HashSet<&'a PackageName>,
) -> Result<()> {
    if order.contains(dep.package_name()) {
        return Ok(());
    }

    // Visit any unresolved foreign dependencies
    match dep {
        DecodedDependency::Wit {
            package,
            resolution,
        } => {
            for name in package.main.foreign_deps.keys() {
                // Only visit known dependencies
                // wit-parser will error on unknown foreign dependencies when
                // the package is resolved
                if let Some(dep) = deps.get(name) {
                    if !visiting.insert(name) {
                        anyhow::bail!("foreign dependency `{name}` forms a dependency cycle while parsing dependency `{other}`", other = resolution.name());
                    }

                    visit(dep, deps, order, visiting)?;
                    assert!(visiting.remove(name));
                }
            }
        }
        DecodedDependency::Wasm {
            decoded,
            resolution,
        } => {
            // Look for foreign packages in the decoded dependency
            for (_, package) in &decoded.resolve().packages {
                if package.name.namespace == dep.package_name().namespace
                    && package.name.name == dep.package_name().name
                {
                    continue;
                }

                if let Some(dep) = deps.get(&package.name) {
                    if !visiting.insert(&package.name) {
                        anyhow::bail!("foreign dependency `{name}` forms a dependency cycle while parsing dependency `{other}`", name = package.name, other = resolution.name());
                    }

                    visit(dep, deps, order, visiting)?;
                    assert!(visiting.remove(&package.name));
                }
            }
        }
    }

    assert!(order.insert(dep.package_name().clone()));

    Ok(())
}
