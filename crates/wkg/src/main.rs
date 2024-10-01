use std::{io::Seek, path::PathBuf};

use anyhow::{ensure, Context};
use clap::{Args, Parser, Subcommand, ValueEnum};
use futures_util::TryStreamExt;
use tokio::io::AsyncWriteExt;
use tracing::level_filters::LevelFilter;
use wasm_pkg_client::{
    caching::{CachingClient, FileCache},
    Client, PublishOpts,
};
use wasm_pkg_common::{
    self,
    config::{Config, RegistryMapping},
    package::PackageSpec,
    registry::Registry,
};
use wit_component::DecodedWasm;

mod oci;
mod wit;

use oci::OciCommands;
use wit::WitCommands;

#[derive(Parser, Debug)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Args, Debug)]
struct RegistryArgs {
    /// The registry domain to use. Overrides configuration file(s).
    #[arg(long = "registry", value_name = "REGISTRY", env = "WKG_REGISTRY")]
    registry: Option<Registry>,
}

#[derive(Args, Debug)]
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
            dirs::cache_dir().context("unable to find cache directory")?
        };
        let dir = dir.join("wkg");
        FileCache::new(dir).await
    }

    /// Helper for loading a caching client. This should be the most commonly used method for
    /// loading a client, but if you need to modify the config or use your own cache, you can use
    /// the [`Common::load_config`] and [`Common::load_cache`] methods.
    pub async fn get_client(&self) -> anyhow::Result<CachingClient<FileCache>> {
        let config = self.load_config().await?;
        let cache = self.load_cache().await?;
        let client = Client::new(config);
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
            Config::global_config_path()
                .ok_or(anyhow::anyhow!("global config path not available"))?
        };

        if self.edit {
            let editor = std::env::var("EDITOR").or(Err(anyhow::anyhow!(
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
            Err(err) => return Err(anyhow::anyhow!("error reading config file: {0}", err)),
        };

        if let Some(default) = self.default_registry {
            // set default registry
            config.set_default_registry(Some(default));

            // write config file
            config.to_file(&path).await?;
            println!("Updated config file: {path}", path = path.display());
        }

        // print config
        if let Some(registry) = config.default_registry() {
            println!("Default registry: {}", registry);
        } else {
            println!("Default registry is not set");
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
    /// The file to publish
    file: PathBuf,

    #[command(flatten)]
    registry_args: RegistryArgs,

    /// If not provided, the package name and version will be inferred from the Wasm file.
    /// Expected format: `<namespace>:<name>@<version>`
    #[arg(long, env = "WKG_PACKAGE")]
    package: Option<PackageSpec>,

    #[command(flatten)]
    common: Common,
}

impl PublishArgs {
    pub async fn run(self) -> anyhow::Result<()> {
        let client = self.common.get_client().await?;

        let package = if let Some(package) = self.package {
            Some((
                package.package,
                package.version.ok_or_else(|| {
                    anyhow::anyhow!("version is required when manually overriding the package ID")
                })?,
            ))
        } else {
            None
        };
        let (package, version) = client
            .client()?
            .publish_release_file(
                &self.file,
                PublishOpts {
                    package,
                    registry: self.registry_args.registry,
                },
            )
            .await?;
        println!("Published {}@{}", package, version);
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
                println!("No version specified; fetching version list...");
                let versions = client.list_all_versions(&package).await?;
                tracing::trace!(?versions, "Fetched version list");
                versions
                    .into_iter()
                    .filter_map(|vi| (!vi.yanked).then_some(vi.version))
                    .max()
                    .context("No releases found")?
            }
        };

        println!("Getting {package}@{version}...");
        let release = client
            .get_release(&package, &version)
            .await
            .context("Failed to get release details")?;
        tracing::debug!(?release, "Fetched release details");

        let output_trailing_slash = self.output.as_os_str().to_string_lossy().ends_with('/');
        let parent_dir = if output_trailing_slash {
            self.output.as_path()
        } else {
            self.output
                .parent()
                .context("Failed to resolve output parent dir")?
        };

        let (tmp_file, tmp_path) =
            tempfile::NamedTempFile::with_prefix_in(".wkg-get", parent_dir)?.into_parts();
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
                    println!(
                        "Couldn't infer output format from file name {:?}",
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
                    Some(wit_component::WitPrinter::default().print(&resolve, pkg, &[])?)
                }
                Ok(_) => None,
                Err(err) => {
                    tracing::debug!(?err, "failed to decode WIT package");
                    if format == Format::Wit {
                        return Err(err);
                    }
                    println!("Failed to detect package content type: {err:#}");
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
        ensure!(
            self.overwrite || !output_path.exists(),
            "{output_path:?} already exists; you can use '--overwrite' to overwrite it"
        );

        if let Some(wit) = wit {
            std::fs::write(&output_path, wit)
                .with_context(|| format!("Failed to write WIT to {output_path:?}"))?
        } else {
            tmp_path
                .persist(&output_path)
                .with_context(|| format!("Failed to persist WASM to {output_path:?}"))?
        }
        println!("Wrote '{}'", output_path.display());

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

    match cli.command {
        Commands::Config(args) => args.run().await,
        Commands::Get(args) => args.run().await,
        Commands::Publish(args) => args.run().await,
        Commands::Oci(args) => args.run().await,
        Commands::Wit(args) => args.run().await,
    }
}
