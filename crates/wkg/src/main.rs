use std::{
    io::{Cursor, Seek},
    path::PathBuf,
};

use anstream::eprintln;
use anyhow::{Context, anyhow, bail, ensure};
use clap::{Args, Parser, Subcommand, ValueEnum};
use futures_util::TryStreamExt;
use tokio::io::AsyncWriteExt;
use tracing::level_filters::LevelFilter;
use wasm_pkg_client::{
    Client, PackageRef, PublishOpts, Version,
    caching::{CachingClient, FileCache},
};
use wasm_pkg_common::{
    self,
    config::{Config, RegistryMapping},
    package::PackageSpec,
    registry::Registry,
};
use wasm_pkg_core::{
    lock::LockFile,
    manifest::{Manifest, workspace::WorkspaceRootConfig},
};
use wit_component::DecodedWasm;

mod oci;
mod overlay;
mod wit;

use oci::OciCommands;
use wit::{BuildArgs, FetchArgs, UpdateArgs, WitCommands};

use crate::{overlay::PublishVerifier, wit::temp_wit_file};

#[macro_export]
macro_rules! warnln {
    ($($arg:tt)*) => {{
        let style = ::anstyle::AnsiColor::Yellow.on_default() | ::anstyle::Effects::BOLD;
        ::anstream::eprintln!("{style}warning{style:#}: {}", format_args!($($arg)*));
    }};
}

#[macro_export]
macro_rules! statusln {
    ($label:expr, $($arg:tt)*) => {{
        let style = ::anstyle::AnsiColor::Cyan.on_default() | ::anstyle::Effects::BOLD;
        ::anstream::eprintln!(
            "{style}{:>12}{style:#} {}",
            $label,
            format_args!($($arg)*),
        );
    }};
}

#[macro_export]
macro_rules! helpln {
    ($($arg:tt)*) => {{
        let style = ::anstyle::AnsiColor::Magenta.on_default() | ::anstyle::Effects::BOLD;
        ::anstream::eprintln!("{style}{:>7}{style:#}: {}", "help", format_args!($($arg)*));
    }};
}

#[derive(Parser, Debug)]
#[command(version)]
struct Cli {
    #[command(flatten)]
    color: colorchoice_clap::Color,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Args, Debug)]
struct RegistryArgs {
    /// The registry domain to use. Overrides configuration file(s).
    #[arg(long = "registry", value_name = "REGISTRY", env = "WKG_REGISTRY")]
    registry: Option<Registry>,
}

#[derive(Args, Debug, Default)]
struct Common {
    /// The path to the configuration file.
    #[arg(long = "config", value_name = "CONFIG", env = "WKG_CONFIG_FILE")]
    config: Option<PathBuf>,
    /// The path to the cache directory. Defaults to the system cache directory.
    #[arg(long = "cache", value_name = "CACHE", env = "WKG_CACHE_DIR")]
    cache: Option<PathBuf>,
}

impl Common {
    /// Helper to load the config from the given path
    pub async fn load_config(&self) -> anyhow::Result<Config> {
        if let Some(config_file) = self.config.as_ref() {
            tracing::info!(config = %config_file.display());
            Config::from_file(config_file)
                .await
                .context(format!("error loading config file {config_file:?}"))
        } else {
            Config::global_defaults().await.map_err(anyhow::Error::from)
        }
    }

    /// Helper for loading the [`FileCache`]
    pub async fn load_cache(&self) -> anyhow::Result<FileCache> {
        let dir = if let Some(dir) = self.cache.as_ref() {
            dir.clone()
        } else {
            FileCache::global_cache_path().context("unable to find cache directory")?
        };

        FileCache::new(dir).await
    }

    /// Helper for loading a caching client. This should be the most commonly used method for
    /// loading a client, but if you need to modify the config or use your own cache, you can use
    /// the [`Common::load_config`] and [`Common::load_cache`] methods.
    pub async fn get_client(&self) -> anyhow::Result<CachingClient<FileCache>> {
        let config = self.load_config().await?;
        let cache = self.load_cache().await?;
        let client = Client::new(config);

        tracing::debug!(filecache_dir = %cache);
        Ok(CachingClient::new(Some(client), cache))
    }
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Set registry configuration
    Config(ConfigArgs),
    /// Download a package from a registry
    Get(GetArgs),
    /// Publish a package to a registry
    Publish(PublishArgs),
    /// Commands for interacting with OCI registries
    #[clap(subcommand)]
    Oci(OciCommands),
    Build(BuildArgs),
    Fetch(FetchArgs),
    Update(UpdateArgs),
    /// Commands for interacting with WIT files and dependencies
    #[clap(subcommand)]
    Wit(WitCommands),
}

#[derive(Args, Debug)]
struct ConfigArgs {
    /// The default registry domain to use. Overrides configuration file(s).
    #[arg(long = "default-registry", value_name = "DEFAULT_REGISTRY")]
    default_registry: Option<Registry>,

    /// Opens the global configuration file in an editor defined in the `$EDITOR` environment variable.
    #[arg(long, short, action)]
    edit: bool,

    #[command(flatten)]
    common: Common,
}

impl ConfigArgs {
    pub async fn run(self) -> anyhow::Result<()> {
        // use config path provided, otherwise global config path
        let path = if let Some(path) = self.common.config {
            path
        } else {
            Config::global_config_path().ok_or(anyhow!("global config path not available"))?
        };

        // Check if the parent directory exists, if not create it
        if let Some(parent) = path.parent() {
            match tokio::fs::metadata(parent).await {
                Ok(metadata) => {
                    if !metadata.is_dir() {
                        anyhow::bail!("parent directory is not a directory");
                    }
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        tokio::fs::create_dir_all(parent).await?;
                    } else {
                        anyhow::bail!("failed to check for config file directory: {}", e);
                    }
                }
            }
        }

        if self.edit {
            let editor = std::env::var("EDITOR").or(Err(anyhow!(
                "failed to read `$EDITOR` environment variable"
            )))?;

            // create file if it doesn't exist
            if !path.is_file() {
                Config::default().to_file(&path).await?;
            }

            // launch editor
            tokio::process::Command::new(editor)
                .arg(&path)
                .status()
                .await
                .context("failed to launch editor")?;

            return Ok(());
        }

        // read file or use default config (not empty config)
        let mut config = match tokio::fs::read_to_string(&path).await {
            Ok(contents) => Config::from_toml(&contents)?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(err) => return Err(anyhow!("error reading config file: {0}", err)),
        };

        if let Some(default) = self.default_registry {
            // set default registry
            config.set_default_registry(Some(default));

            // write config file
            config.to_file(&path).await?;
            eprintln!("Updated config file: {path}", path = path.display());
        }

        // print config
        if let Some(registry) = config.default_registry() {
            eprintln!("Default registry: {}", registry);
        } else {
            warnln!("Default registry is not set");
        }

        Ok(())
    }
}

#[derive(Args, Debug)]
struct GetArgs {
    /// Output path. If this ends with a '/', a filename based on the package
    /// name, version, and format will be appended, e.g.
    /// `name-space_name@1.0.0.wasm``.
    #[arg(long, short, default_value = "./")]
    output: PathBuf,

    /// Output format. The default of "auto" detects the format based on the
    /// output filename or package contents.
    #[arg(long, value_enum, default_value = "auto")]
    format: Format,

    /// Check that the retrieved package matches the existing file at the
    /// output path. Output path will not be modified. Program exits with
    /// codes similar to diff(1): exits with 1 if there were differences, and
    /// 0 means no differences.
    #[arg(long, conflicts_with = "overwrite")]
    check: bool,

    /// Overwrite any existing output file.
    #[arg(long)]
    overwrite: bool,

    /// The package to get, specified as `<namespace>:<name>` plus optional
    /// `@<version>`, e.g. `wasi:cli" or `wasi:http@0.2.0`.
    package_spec: PackageSpec,

    #[command(flatten)]
    registry_args: RegistryArgs,

    #[command(flatten)]
    common: Common,
}

#[derive(Args, Debug)]
struct PublishArgs {
    /// The files and directories to publish.
    /// If a directory is provided, the package is built to a tempfile before publishing.
    paths: Vec<PathBuf>,

    #[command(flatten)]
    registry_args: RegistryArgs,

    /// If not provided, the package name and version will be inferred from the Wasm file.
    /// Expected format: `<namespace>:<name>@<version>`
    #[arg(long, env = "WKG_PACKAGE")]
    package: Option<PackageSpec>,

    /// Attempt package, version, and registry resolution without publishing.
    #[arg(long)]
    dry_run: bool,

    /// Publish all packages in the workspace
    #[arg(long)]
    workspace: bool,

    /// Disable semver compatibility checks.
    #[arg(long)]
    skip_semver_check: bool,

    /// Skip publishing any package whose `(name, version)` already exists on
    /// the target registry. A successful `get_release` probe short-circuits the
    /// publish; probe failures fall through to a normal publish attempt.
    #[arg(long)]
    skip_dupes: bool,

    #[command(flatten)]
    common: Common,
}

impl PublishArgs {
    pub async fn run(mut self) -> anyhow::Result<()> {
        let publish_opts = self.publish_opts()?;
        let _root = self.workspace_root().await?;
        let path = match &self.paths[..] {
            [] => {
                anyhow::bail!(
                    "no publish targets: pass one or more paths, or run from a workspace \
                     (see `[workspace] members` in `wkg.toml`)"
                );
            }
            [path] => path,
            paths => {
                // Route every package in the plan to the local overlay registry
                // backed by `reg_config`, so the client used in `build_wit_dir`
                // resolves these packages against the local overlay instead of
                // an upstream remote.
                let mut lock_file = LockFile::load(false).await?;
                let verifier = PublishVerifier::try_new(
                    paths,
                    "tmp_local_publish",
                    self.common.load_config().await?,
                    self.common.load_cache().await?,
                    &mut lock_file,
                    true,
                )
                .await?;
                let mut plan = verifier.plan;
                anstream::eprintln!("{plan}");

                let client = self.common.get_client().await?;
                // Publish our packages in "waves" to the actual registries ensuring all
                // possible dependency free packages are published in the same group
                while !plan.is_empty() {
                    // `ready_for_publish` is guaranteed to be nonempty IF `plan.is_empty() == false`
                    //
                    // A `DependencyGraph` (`petgraph::Acyclic`) should always hold valid edges.
                    // Any insertions to the `DependencyGraph` that would produce invalid edges should
                    // result in an error when calling `try_update_edge` inside `wasm_pkg_core::wit::get_local_dependencies`
                    let ready_for_publish = plan.take_ready();
                    for spec in &ready_for_publish {
                        let data = verifier
                            .data
                            .get(&spec.package)
                            .expect("missing package ref");
                        let source = Box::pin(Cursor::new(data.clone()));
                        // we do not have guarantees that the underlying `PackagePublisher::publish`
                        // will terminate
                        let res = client
                            .client()?
                            .publish_release_data(source, publish_opts.clone())
                            .await;
                        self.handle_publish_result(res).context(spec.clone())?;
                    }
                    plan.mark_confirmed(ready_for_publish);
                }
                return Ok(());
            }
        };

        let client = self.common.get_client().await?;

        // If the input is a directory, build a WIT package from it into a temp
        // file first. _tmp is held until the publish completes so the file
        // isn't deleted out from under us.
        let (publish_path, _tmp) = if path.is_dir() {
            let mut lock_file = LockFile::load(true).await?;
            let prev_lock_ref = (lock_file.version, lock_file.packages.clone());
            let (pkg_ref, _, bytes) =
                wit::build_wit_dir(&path.clone(), client.clone(), &mut lock_file).await?;
            // There is no way to check if we are in a git repository unlike `cargo publish --allow-dirty` so
            // check against previous values.
            if lock_file != prev_lock_ref && !self.dry_run {
                return Err(anyhow!(
                    "wkg.lock would be updated during publish, aborting"
                ))
                .with_context(|| {
                    format!(
                        "Run `wkg wit build {}` before attempting to publish",
                        path.display()
                    )
                });
            }

            let tmp = temp_wit_file(&pkg_ref, &bytes).await?;

            (tmp.path().to_path_buf(), Some(tmp))
        } else {
            (path.clone(), None)
        };

        let res = client
            .client()?
            .publish_release_file(&publish_path, publish_opts)
            .await;
        self.handle_publish_result(res)?;

        Ok(())
    }

    fn publish_opts(&mut self) -> anyhow::Result<PublishOpts> {
        let package = match self.package.clone() {
            Some(_) if self.paths.len() != 1 => {
                anyhow::bail!(
                    "`--package` is currently only supported when providing one path argument"
                );
            }
            Some(PackageSpec {
                package,
                version: Some(v),
            }) => Some((package, v)),
            Some(PackageSpec { version: None, .. }) => {
                anyhow::bail!("version is required when manually overriding the package ID");
            }
            None => None,
        };

        Ok(PublishOpts {
            package,
            registry: self.registry_args.registry.clone(),
            dry_run: self.dry_run,
            skip_semver_check: self.skip_semver_check,
        })
    }
    async fn workspace_root(&mut self) -> anyhow::Result<Option<WorkspaceRootConfig>> {
        match self.workspace {
            true if !self.paths.is_empty() => anyhow::bail!(
                "`--workspace` selects every workspace member; do not also pass explicit \
                     path arguments"
            ),

            true => {}
            false => return Ok(None),
        }
        let cwd = std::env::current_dir()?;
        let Some(root) = Manifest::load_root_workspace(&cwd).await? else {
            bail!(
                "`--workspace` called but unable to find workspace root from {}",
                cwd.display(),
            )
        };
        self.paths = root.members.clone();
        Ok(Some(root))
    }

    fn handle_publish_result(
        &self,
        res: Result<(PackageRef, Version), wasm_pkg_common::Error>,
    ) -> Result<(), anyhow::Error> {
        match res {
            Err(e @ wasm_pkg_common::Error::VersionAlreadyExists(..)) if self.skip_dupes => {
                warnln!("Skipping publish: {e}");
            }
            Ok((package, version)) => {
                if self.dry_run {
                    warnln!("Aborting publish due to dry run: {}@{}", package, version);
                } else {
                    eprintln!("Published {}@{}", package, version);
                }
            }
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }
}

#[derive(ValueEnum, Clone, Debug, PartialEq)]
enum Format {
    Auto,
    Wasm,
    Wit,
}

impl GetArgs {
    pub async fn run(self) -> anyhow::Result<()> {
        let PackageSpec { package, version } = self.package_spec;
        let mut config = self.common.load_config().await?;
        if let Some(registry) = self.registry_args.registry.clone() {
            tracing::debug!(%package, %registry, "overriding package registry");
            config.set_package_registry_override(
                package.clone(),
                RegistryMapping::Registry(registry),
            );
        }
        let client = Client::new(config);
        let cache = self.common.load_cache().await?;
        let client = CachingClient::new(Some(client), cache);

        let version = match version {
            Some(ver) => ver,
            None => {
                warnln!("no version specified; fetching version list...");
                statusln!("Fetching", "version list");
                let versions = client.list_all_versions(&package).await?;
                tracing::trace!(?versions, "Fetched version list");
                versions
                    .into_iter()
                    .filter_map(|vi| (!vi.yanked).then_some(vi.version))
                    .max()
                    .context("No releases found")?
            }
        };

        statusln!("Fetching", "{package}@{version}...");
        let release = client
            .get_release(&package, &version)
            .await
            .context("Failed to get release details")?;
        tracing::debug!(?release, "Fetched release details");

        let output_trailing_slash = self.output.as_os_str().to_string_lossy().ends_with('/');
        let parent_dir_path = if output_trailing_slash {
            // TODO(fix): TOCTOU on directory, requires platform open dir flags
            let parent_dir_path = self.output.as_path();
            if !tokio::fs::try_exists(parent_dir_path).await? {
                tokio::fs::create_dir_all(parent_dir_path)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to create output dir @ [{}]",
                            parent_dir_path.display()
                        )
                    })?;
            }
            parent_dir_path
        } else {
            self.output
                .parent()
                .context("Failed to resolve non-trailing-slash output parent dir")?
        };

        ensure!(
            parent_dir_path.exists(),
            "output directory {:?} does not exist; create it first or choose a different path",
            parent_dir_path
        );

        let (tmp_file, tmp_path) =
            tempfile::NamedTempFile::with_prefix_in(".wkg-get", parent_dir_path)?.into_parts();
        tracing::debug!(?tmp_path, "Created temporary file");

        let mut content_stream = client.get_content(&package, &release).await?;

        let mut file = tokio::fs::File::from_std(tmp_file);
        while let Some(chunk) = content_stream.try_next().await? {
            file.write_all(&chunk).await?;
        }

        let mut format = self.format;
        if let (Format::Auto, Some(ext)) = (&format, self.output.extension()) {
            tracing::debug!(?ext, "Inferring output format from file extension");
            format = match ext.to_string_lossy().as_ref() {
                "wasm" => Format::Wasm,
                "wit" => Format::Wit,
                _ => {
                    warnln!(
                        "couldn't infer output format from file name {:?}",
                        self.output.file_name().unwrap_or_default()
                    );
                    Format::Auto
                }
            }
        }

        let wit = if format == Format::Wasm {
            None
        } else {
            let mut file = file.into_std().await;
            file.rewind()?;
            match wit_component::decode_reader(&mut file) {
                Ok(DecodedWasm::WitPackage(resolve, pkg)) => {
                    tracing::debug!(?pkg, "decoded WIT package");
                    let mut printer = wit_component::WitPrinter::default();
                    printer.print(&resolve, pkg, &[])?;
                    Some(printer.output.to_string())
                }
                Ok(_) => None,
                Err(err) => {
                    warnln!("unable to decode WIT package");
                    if format == Format::Wit {
                        return Err(err);
                    }
                    eprintln!("failed to detect package content type: {err:#}");
                    None
                }
            }
        };

        let output_path = if output_trailing_slash {
            let ext = if wit.is_some() { "wit" } else { "wasm" };
            self.output.join(format!(
                "{namespace}_{name}@{version}.{ext}",
                namespace = package.namespace(),
                name = package.name(),
            ))
        } else {
            self.output
        };

        if self.check {
            let existing = tokio::fs::read(&output_path)
                .await
                .with_context(|| format!("Failed to read {output_path:?}"))?;
            let latest = if let Some(wit) = wit {
                wit.into_bytes()
            } else {
                tokio::fs::read(&tmp_path)
                    .await
                    .with_context(|| format!("Failed to read {tmp_path:?}"))?
            };
            if existing != latest {
                anyhow::bail!("Differences between retrieved and {output_path:?}");
            }
        } else {
            ensure!(
                self.overwrite || !output_path.exists(),
                "{output_path:?} already exists; you can use '--overwrite' to overwrite it"
            );

            if let Some(wit) = wit {
                tokio::fs::write(&output_path, wit)
                    .await
                    .with_context(|| format!("Failed to write WIT to {output_path:?}"))?
            } else {
                tmp_path
                    .persist(&output_path)
                    .with_context(|| format!("Failed to persist WASM to {output_path:?}"))?
            }
            eprintln!("Wrote '{}'", output_path.display());
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(LevelFilter::WARN.into())
                .from_env_lossy(),
        )
        .init();

    let cli = Cli::parse();
    cli.color.write_global();

    match cli.command {
        Commands::Config(args) => args.run().await,
        Commands::Get(args) => args.run().await,
        Commands::Publish(args) => args.run().await,
        Commands::Oci(args) => args.run().await,
        Commands::Build(args) => args.run().await,
        Commands::Fetch(args) => args.run().await,
        Commands::Update(args) => args.run().await,
        Commands::Wit(args) => {
            warnln!("`wkg wit <command>` is deprecated");
            helpln!("use `wkg <command>` instead");
            args.run().await
        }
    }
}
