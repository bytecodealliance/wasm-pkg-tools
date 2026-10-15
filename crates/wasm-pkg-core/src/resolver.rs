//! A resolver for resolving dependencies from a component registry.
// NOTE(thomastaylor312): This is copied and adapted from the `cargo-component` crate: https://github.com/bytecodealliance/cargo-component/blob/f0be1c7d9917aa97e9102e69e3b838dae38d624b/crates/core/src/registry.rs

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, hash_map},
    fmt::Debug,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, bail};
use futures_util::TryStreamExt;
use indexmap::IndexMap;
use petgraph::{
    Direction, acyclic::Acyclic, data::Build, graph::NodeIndex, stable_graph::StableDiGraph,
};
use semver::{Comparator, Op, Version, VersionReq};
use tokio::io::{AsyncRead, AsyncReadExt};
use wasm_pkg_client::{
    Client, Config, ContentDigest, Error as WasmPkgError, PackageRef, Release, VersionInfo,
    caching::{CachingClient, FileCache},
};
use wasm_pkg_common::package::{PackageReq, PackageSpec};
use wit_component::DecodedWasm;
use wit_parser::{PackageId, PackageName, Resolve, UnresolvedPackageGroup, WorldId};

use crate::{
    lock::LockFile,
    wit::{get_local_dependencies, get_packages},
};

/// The name of the default registry.
pub const DEFAULT_REGISTRY_NAME: &str = "default";

/// Graph of packages where an edge `a -> b` means "`a` depends on `b`".
pub type DependencyGraph<N, E = petgraph::Direction> = Acyclic<StableDiGraph<N, E>>;

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
                    "{{registry=\"{registry}\" package=\"{}@{version}\"}}",
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
    /// The version declared by the package at `path`.
    ///
    /// `None` means the package declares no version, which in WIT is a package distinct from any
    /// versioned one.
    pub version: Option<Version>,
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
    /// `None` means the resolved package declares no version, which is only possible for a local
    /// resolution: a registry release always has one.
    pub fn version(&self) -> Option<&Version> {
        match self {
            Self::Registry(res) => Some(&res.version),
            Self::Local(res) => res.version.as_ref(),
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
            let package = UnresolvedPackageGroup::parse(
                // This is fake, but it's needed for the parser to work.
                self.name().to_string(),
                std::str::from_utf8(&bytes).with_context(|| {
                    format!(
                        "dependency `{name}` is not UTF-8 encoded",
                        name = self.name()
                    )
                })?,
            )
            .map_err(|(src_map, parse_err)| {
                anyhow::format_err!(
                    "failed to parse package group: {}",
                    parse_err.render(&src_map)
                )
            })?;

            return Ok(DecodedDependency::Wit {
                resolution: self,
                package,
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
    /// Overrides keyed by the requirement they stand in for. Overlaps are rejected up front, so
    /// at most one matches any requirement.
    overrides: BTreeMap<PackageReq, Dependency>,
    /// Requirements from the root WIT package and, transitively, each local package pulled in.
    requirements: BTreeSet<PackageReq>,
}

impl<'a> DependencyResolver<'a> {
    /// Creates a new dependency resolver. If [`Config`] is `None`, then the resolver will be set to
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
            packages: Default::default(),
            overrides: Default::default(),
            requirements: Default::default(),
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
            packages: Default::default(),
            overrides: Default::default(),
            requirements: Default::default(),
        })
    }

    /// Add manifest overrides to the resolver.
    ///
    /// Overrides for the same package may name disjoint versions but must not overlap, so the
    /// outcome does not depend on the order they arrive in.
    pub fn add_overrides(
        &mut self,
        overrides: impl IntoIterator<Item = (PackageReq, Dependency)>,
    ) -> Result<()> {
        let overrides: Vec<(PackageReq, Dependency)> = overrides.into_iter().collect();

        let mut conflicts: BTreeSet<String> = BTreeSet::new();
        for (i, (key_a, _)) in overrides.iter().enumerate() {
            for (key_b, _) in overrides.iter().skip(i + 1) {
                if key_a.intersects(key_b) {
                    let (a, b) = if key_a.to_string() <= key_b.to_string() {
                        (key_a, key_b)
                    } else {
                        (key_b, key_a)
                    };
                    conflicts.insert(format!("`{a}` and `{b}`"));
                }
            }
        }
        if !conflicts.is_empty() {
            bail!(
                "overrides must not overlap: {} - remove one from each pair",
                conflicts.into_iter().collect::<Vec<_>>().join("; ")
            );
        }

        for (key, dependency) in overrides {
            self.overrides.insert(key, dependency);
        }
        Ok(())
    }

    /// Add the version requirements named by a WIT file (or package) to the resolver.
    pub fn add_requirements(&mut self, requirements: impl IntoIterator<Item = PackageReq>) {
        self.requirements.extend(requirements);
    }

    /// Resolve all requirements.
    ///
    /// This will download all dependencies that are not already present in client storage. The
    /// result does not depend on the order requirements or overrides were added in.
    pub async fn resolve(mut self) -> Result<DependencyResolutionMap> {
        let mut map = DependencyResolutionMap::default();
        // Requirements that resolve to the same release or local path share one node.
        let mut nodes: HashMap<ResolutionIdentity, NodeIndex> = HashMap::new();
        // Local packages, cached so a path is only parsed once.
        let mut local_packages: HashMap<PathBuf, (PackageSpec, BTreeSet<PackageReq>)> =
            HashMap::new();
        let overrides = std::mem::take(&mut self.overrides);

        // Each entry carries the node that depends on the requirement, so an edge can be drawn
        // once the requirement's own node exists.
        let mut worklist: Vec<(PackageReq, Option<NodeIndex>)> =
            std::mem::take(&mut self.requirements)
                .into_iter()
                .map(|req| (req, None))
                .collect();

        while let Some((req, parent)) = worklist.pop() {
            let matched = overrides
                .iter()
                .find(|(key, _)| key.intersects(&req))
                .map(|(key, dep)| (key.clone(), dep.clone()));

            let (identity, resolution, discovered) = match matched {
                Some((key, Dependency::Local(path))) => {
                    if !local_packages.contains_key(&path) {
                        let entry = get_packages(&path).with_context(|| {
                            format!(
                                "Error getting packages from local dependency {}",
                                path.display()
                            )
                        })?;
                        local_packages.insert(path.clone(), entry);
                    }
                    let (found, packages) = local_packages.get(&path).expect("just inserted");

                    // The package on disk must satisfy the requirement that reached it.
                    if !req.matches(found) {
                        bail!(
                            "override `{key}` points at {path}, which provides `{found}`, but it is \
                             required as `{req}`",
                            path = path.display(),
                        );
                    }

                    (
                        ResolutionIdentity::Local(path.clone()),
                        DependencyResolution::Local(LocalResolution {
                            name: found.package.clone(),
                            version: found.version.clone(),
                            path: path.clone(),
                        }),
                        packages.iter().cloned().collect::<Vec<_>>(),
                    )
                }
                Some((_key, Dependency::Package(package))) => {
                    let name = req.package.clone();
                    let package_name = package.name.clone().unwrap_or_else(|| name.clone());
                    let res = self
                        .resolve_registry(
                            &name,
                            package_name,
                            package.version.clone(),
                            package.registry.as_deref(),
                        )
                        .await?;
                    (
                        ResolutionIdentity::from_registry(&res),
                        DependencyResolution::Registry(res),
                        Vec::new(),
                    )
                }
                None => {
                    let name = req.package.clone();
                    let res = self
                        .resolve_registry(&name, name.clone(), req.version.clone(), None)
                        .await?;
                    (
                        ResolutionIdentity::from_registry(&res),
                        DependencyResolution::Registry(res),
                        Vec::new(),
                    )
                }
            };

            let node = match nodes.get(&identity) {
                Some(&node) => node,
                None => {
                    let node = map.graph.add_node(resolution);
                    nodes.insert(identity, node);
                    for dep in discovered {
                        worklist.push((dep, Some(node)));
                    }
                    node
                }
            };

            map.requirements.insert(req.clone(), node);

            // `try_update_edge` keeps the graph acyclic and rejects self-loops.
            if let Some(parent) = parent {
                map.graph
                    .try_update_edge(parent, node, ())
                    .map_err(|e| match e {
                        petgraph::acyclic::AcyclicEdgeError::Cycle(cycle) => {
                            anyhow::anyhow!("cyclical dependency detected").context(format!(
                                "other package: {}",
                                map.graph[cycle.node_id()].name()
                            ))
                        }
                        petgraph::acyclic::AcyclicEdgeError::SelfLoop => {
                            anyhow::anyhow!("package `{req}` depends on itself")
                        }
                        petgraph::acyclic::AcyclicEdgeError::InvalidEdge => anyhow::anyhow!(
                            "could not successfully add the edge to the dependency graph"
                        ),
                    })?;
            }
        }

        Ok(map)
    }

    /// Resolve a single registry package, selecting a release that satisfies `version` and
    /// consulting the lock file if one is present.
    async fn resolve_registry(
        &mut self,
        name: &PackageRef,
        package: PackageRef,
        version: VersionReq,
        registry: Option<&str>,
    ) -> Result<RegistryResolution> {
        // Clone the client so `self` can be borrowed mutably below.
        let client = self.client.clone();

        let registry_name = registry.or_else(|| {
            self.client.client().ok().and_then(|client| {
                client
                    .config()
                    .resolve_registry(name)
                    .map(|reg| reg.as_ref())
            })
        });

        let locked = match self.lock_file.as_ref().and_then(|resolver| {
            resolver
                .resolve(registry_name, &package, &version)
                .transpose()
        }) {
            Some(Ok(locked)) => Some(locked),
            Some(Err(e)) => return Err(e),
            _ => None,
        };

        let (selected_version, digest) = if client.is_readonly() {
            locked
                .as_ref()
                .map(|l| (&l.version, Some(&l.digest)))
                .ok_or_else(|| {
                    anyhow::anyhow!("Couldn't find locked dependency while in offline mode")
                })?
        } else {
            let versions = load_package(&mut self.packages, &self.client, package.clone())
                .await
                .with_context(|| format!("package: {}", package.clone()))?
                .with_context(|| {
                    format!("package `{package}` was not found in component registry")
                })?;

            match &locked {
                Some(locked) => {
                    // Try an exact match against the locked version first.
                    let locked_version = &locked.version;
                    let exact_req = VersionReq {
                        comparators: vec![Comparator {
                            op: Op::Exact,
                            major: locked_version.major,
                            minor: Some(locked_version.minor),
                            patch: Some(locked_version.patch),
                            pre: locked_version.pre.clone(),
                        }],
                    };

                    // Fall back to the latest release if the locked one was yanked, carrying the
                    // locked digest along for comparison after the fetch.
                    find_latest_release(versions, &exact_req)
                        .map(|v| (&v.version, Some(&locked.digest)))
                        .or_else(|| find_latest_release(versions, &version).map(|v| (&v.version, None)))
                }
                None => find_latest_release(versions, &version).map(|v| (&v.version, None)),
            }
            .with_context(|| {
                format!(
                    "component registry package `{package}` has no release matching version requirement `{version}`"
                )
            })?
        };

        let release = client.get_release(&package, selected_version).await?;
        if let Some(digest) = digest
            && &release.content_digest != digest
        {
            bail!(
                "component registry package `{package}` (v`{version}`) has digest `{content}` but the lock file specifies digest `{digest}`",
                version = release.version,
                content = release.content_digest,
            );
        }

        let registry = self.client.client().ok().and_then(|client| {
            client
                .config()
                .resolve_registry(name)
                .map(ToString::to_string)
        });

        Ok(RegistryResolution {
            name: name.clone(),
            package,
            registry,
            requirement: version,
            version: release.version.clone(),
            digest: release.content_digest.clone(),
            client: self.client.clone(),
        })
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

/// Identity of a resolved package, so requirements selecting the same release or local path share
/// one graph node.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ResolutionIdentity {
    Registry {
        package: PackageRef,
        version: Version,
        registry: Option<String>,
    },
    Local(PathBuf),
}

impl ResolutionIdentity {
    fn from_registry(res: &RegistryResolution) -> Self {
        Self::Registry {
            package: res.package.clone(),
            version: res.version.clone(),
            registry: res.registry.clone(),
        }
    }

    fn of(resolution: &DependencyResolution) -> Self {
        match resolution {
            DependencyResolution::Registry(res) => Self::from_registry(res),
            DependencyResolution::Local(res) => Self::Local(res.path.clone()),
        }
    }
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

/// Resolutions chosen for a set of requirements.
///
/// Stored in a [`DependencyGraph`] where an edge `a -> b` means "`a` depends on `b`". Each
/// requirement maps to exactly one node; a node may satisfy several requirements.
#[derive(Debug, Clone, Default)]
pub struct DependencyResolutionMap {
    graph: DependencyGraph<DependencyResolution, ()>,
    requirements: BTreeMap<PackageReq, NodeIndex>,
}

impl DependencyResolutionMap {
    /// The resolution chosen for `requirement`, if it was resolved.
    pub fn get(&self, requirement: &PackageReq) -> Option<&DependencyResolution> {
        self.requirements
            .get(requirement)
            .map(|id| &self.graph[*id])
    }

    /// Every requirement and the resolution chosen for it.
    pub fn requirements(&self) -> impl Iterator<Item = (&PackageReq, &DependencyResolution)> {
        self.requirements
            .iter()
            .map(|(req, id)| (req, &self.graph[*id]))
    }

    /// Every distinct resolution, dependencies before their dependents.
    pub fn resolutions(&self) -> impl Iterator<Item = &DependencyResolution> {
        // `nodes_iter` orders a node before its successors; reverse to put dependencies first.
        let mut nodes: Vec<NodeIndex> = self.graph.nodes_iter().collect();
        nodes.reverse();
        nodes.into_iter().map(|id| &self.graph[id])
    }

    /// The number of distinct resolutions.
    pub fn len(&self) -> usize {
        self.graph.node_count()
    }

    /// Whether there are no resolutions.
    pub fn is_empty(&self) -> bool {
        self.graph.node_count() == 0
    }

    /// Insert a requirement and the resolution it selects, reusing an existing node for the same
    /// resolution.
    pub fn insert(&mut self, requirement: PackageReq, resolution: DependencyResolution) {
        let identity = ResolutionIdentity::of(&resolution);
        let node = match self
            .graph
            .node_indices()
            .find(|id| ResolutionIdentity::of(&self.graph[*id]) == identity)
        {
            Some(id) => id,
            None => self.graph.add_node(resolution),
        };
        self.requirements.insert(requirement, node);
    }

    /// Merge `other` into `self`, preserving edges. Errors if the result would contain a cycle.
    pub fn extend(&mut self, other: &DependencyResolutionMap) -> Result<()> {
        self.extend_filtered(other, |_| true)
    }

    /// Merge the resolutions of `other` that satisfy `keep`, preserving edges between them.
    /// Requirements and edges of dropped resolutions are dropped as well.
    pub fn extend_filtered(
        &mut self,
        other: &DependencyResolutionMap,
        keep: impl Fn(&DependencyResolution) -> bool,
    ) -> Result<()> {
        // Map each kept node in `other` to the equivalent node here, adding it if missing.
        let mut remap: HashMap<NodeIndex, NodeIndex> = HashMap::new();
        for id in other.graph.node_indices() {
            let resolution = &other.graph[id];
            if !keep(resolution) {
                continue;
            }
            let identity = ResolutionIdentity::of(resolution);
            let node = match self
                .graph
                .node_indices()
                .find(|existing| ResolutionIdentity::of(&self.graph[*existing]) == identity)
            {
                Some(existing) => existing,
                None => self.graph.add_node(resolution.clone()),
            };
            remap.insert(id, node);
        }

        for edge in other.graph.edge_indices() {
            let (a, b) = other
                .graph
                .edge_endpoints(edge)
                .expect("edge index should have endpoints");
            // Skip edges that touch a resolution that was filtered out.
            let (Some(&a), Some(&b)) = (remap.get(&a), remap.get(&b)) else {
                continue;
            };
            self.graph.try_update_edge(a, b, ()).map_err(|e| match e {
                petgraph::acyclic::AcyclicEdgeError::Cycle(cycle) => {
                    anyhow::anyhow!("cyclical dependency detected").context(format!(
                        "other package: {}",
                        self.graph[cycle.node_id()].name()
                    ))
                }
                petgraph::acyclic::AcyclicEdgeError::SelfLoop => {
                    anyhow::anyhow!("package depends on itself")
                }
                petgraph::acyclic::AcyclicEdgeError::InvalidEdge => {
                    anyhow::anyhow!("could not successfully add the edge to the dependency graph")
                }
            })?;
        }

        for (requirement, id) in &other.requirements {
            if let Some(&node) = remap.get(id) {
                self.requirements.insert(requirement.clone(), node);
            }
        }

        Ok(())
    }

    /// Fetch all dependencies, decoded in dependency-first order, ready for output or adding to a
    /// [`Resolve`]. The graph is acyclic by construction.
    pub async fn decode_dependencies(
        &self,
    ) -> Result<IndexMap<PackageName, DecodedDependency<'_>>> {
        let mut deps = IndexMap::new();
        for resolution in self.resolutions() {
            let decoded = resolution.decode().await?;
            if let Some(prev) = deps.insert(decoded.package_name().clone(), decoded) {
                anyhow::bail!(
                    "duplicate definitions of package `{prev}` found while decoding dependency `{name}`",
                    prev = prev.package_name(),
                    name = resolution.name(),
                );
            }
        }

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

/// Mapping of [`PackageRef`]s to the respective index inside the dependency graph.
pub type LocalPackageIndex = HashMap<PackageRef, (NodeIndex, PathBuf)>;

/// State for tracking dependencies during upload.
pub struct PublishPlan {
    /// Graph of publishable packages where the edges are `(dependency -DependencyOf->) dependent)`
    dependents: DependencyGraph<PackageSpec>,
    // TODO look at using cargo's `InternedString` type for `PackageRef`:
    // https://docs.rs/cargo/latest/cargo/util/interning/struct.InternedString.html
    indices: LocalPackageIndex,
}

impl PublishPlan {
    /// Generate [`Self`] from a list of WIT package paths (files or directories).
    pub fn from_paths(paths: &[impl AsRef<Path>]) -> Result<Self> {
        let (graph, indices) = get_local_dependencies(paths)?;
        {
            // collection of local packages that have no version
            let missing_version = graph
                .nodes_iter()
                .map(|f| graph[f].clone())
                .filter(|pkg| pkg.version.is_none())
                .collect::<Vec<_>>();
            if !missing_version.is_empty() {
                return Err(anyhow::anyhow!(
                    "Unable to publish packages without a version specified"
                )
                .context(format!(
                    "packages: {}",
                    package_list(missing_version.iter(), None)
                )));
            }
        }
        let mut dependents = graph.into_inner();

        dependents.reverse();
        // graph was already found to be acyclic
        let dependents = DependencyGraph::try_from(dependents).unwrap();

        Ok(Self {
            dependents,
            indices,
        })
    }

    pub fn iter<'a>(&'a self) -> impl Iterator<Item = &'a PackageSpec> + 'a {
        self.dependents
            .nodes_iter()
            .map(|id| &(self.dependents[id]))
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn len(&self) -> usize {
        self.indices.len()
    }

    /// Returns the set of packages that are ready for publishing (i.e. have no outstanding dependencies).
    ///
    /// These will not be returned in future calls.
    pub fn take_ready(&self) -> BTreeSet<PackageSpec> {
        self.dependents
            .nodes_iter()
            // there are no dependents on `self.dependents[id]`
            .filter(|id| {
                self.dependents
                    .neighbors_directed(*id, Direction::Incoming)
                    .count()
                    == 0
            })
            .map(|id| self.dependents[id].clone())
            .collect()
    }

    /// Return the path associated with a local package.
    pub fn get_path(&self, pkg: &PackageRef) -> Option<&Path> {
        self.indices.get(pkg).map(|(_, p)| p.as_ref())
    }

    /// Return the [`NodeIndex`] associated with a local package.
    pub fn get_node_index(&self, pkg: &PackageRef) -> Option<NodeIndex> {
        self.indices.get(pkg).map(|(id, _)| *id)
    }

    /// Packages confirmed to be available in the registry, potentially allowing additional
    /// packages to be "ready".
    pub fn mark_confirmed(&mut self, published: impl IntoIterator<Item = PackageSpec>) {
        for spec in published {
            let (id, _) = self
                .indices
                .remove(&spec.package)
                .expect("PackageSpec has no associated index");
            // NOTE: nodes without edges will return None here
            self.dependents.remove_node(id);
        }
    }
}

impl std::fmt::Display for PublishPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // TODO(mkatychev): handle with anstyle and anstream, passing in `anstream::AutoStream` for colour choice
        for id in self.dependents.nodes_iter() {
            let dep = &self.dependents[id];
            // initial dependency graph visualization
            let mut neighbors = self
                .dependents
                .neighbors_directed(id, Direction::Outgoing)
                .peekable();

            if neighbors.peek().is_none() {
                // tracing::debug!("{dep} has no dependents");
                writeln!(f, "[{dep} has no dependents]")?;
            } else {
                writeln!(f, "[{dep}]")?;
            }

            while let Some(id) = neighbors.next() {
                let pkg = &self.dependents[id];
                let separator = if neighbors.peek().is_some() {
                    "├─"
                } else {
                    "╰─"
                };

                writeln!(f, "{separator}─▶ {pkg}")?;
            }
        }
        Ok(())
    }
}

/// Format a collection of packages as a list
///
/// e.g. "foo:a@0.1.0, bar:b@0.2.0, and baz:c@0.3.0".
///
/// Note: the final separator (e.g. "and" in the previous example) can be chosen.
fn package_list<'a>(
    pkgs: impl IntoIterator<Item = &'a PackageSpec>,
    final_sep: Option<&str>,
) -> String {
    let final_sep = final_sep.unwrap_or("and");
    let mut names: Vec<_> = pkgs.into_iter().map(|pkg| pkg.to_string()).collect();
    names.sort();

    match &names[..] {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} {final_sep} {b}"),
        [names @ .., last] => {
            format!("{}, {final_sep} {last}", names.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {

    use std::path::PathBuf;

    use super::*;
    use glob::glob;

    fn transitive_local_paths() -> Vec<PathBuf> {
        let fixtures_root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/transitive-local");
        // pass all fixture dirs to a single `wkg publish` invocation
        let mut paths: Vec<PathBuf> = glob(fixtures_root.join("**/*.wit").to_str().unwrap())
            .unwrap()
            .map(|p| p.expect("glob path error"))
            .map(|p| p.parent().unwrap().to_path_buf())
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }

    #[test]
    fn publish_plan_iter() {
        let paths = transitive_local_paths();
        let plan = PublishPlan::from_paths(&paths).unwrap();
        assert_eq!(
            plan.iter().count(),
            5,
            "unexpected package count\npackages found: {}",
            package_list(plan.iter(), None)
        );
    }

    #[test]
    fn publish_plan_chunks() {
        let paths = transitive_local_paths();
        let mut plan = PublishPlan::from_paths(&paths).unwrap();
        let mut ready_for_publish = plan.take_ready();
        assert_eq!(
            ready_for_publish.iter().collect::<Vec<_>>(),
            ["example-c:nested@0.1.0", "example-d:foo@0.1.0",],
        );
        plan.mark_confirmed(ready_for_publish);

        ready_for_publish = plan.take_ready();
        assert_eq!(
            ready_for_publish.iter().collect::<Vec<_>>(),
            ["example-c:baz@0.1.0"],
        );
        plan.mark_confirmed(ready_for_publish);

        ready_for_publish = plan.take_ready();
        assert_eq!(
            ready_for_publish.iter().collect::<Vec<_>>(),
            ["example-b:bar@0.1.0"],
        );
        plan.mark_confirmed(ready_for_publish);

        ready_for_publish = plan.take_ready();
        assert_eq!(
            ready_for_publish.iter().collect::<Vec<_>>(),
            ["example-a:foo@0.1.0"],
        );
        plan.mark_confirmed(ready_for_publish);

        assert!(plan.is_empty());
    }
}
