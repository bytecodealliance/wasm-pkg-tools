use std::{io::Seek, path::PathBuf};

use anyhow::{ensure, Context};
use clap::{Args, Parser, Subcommand, ValueEnum};
use futures_util::TryStreamExt;
use tokio::io::AsyncWriteExt;
use tracing::level_filters::LevelFilter;
use wasm_pkg_common::{config::Config, package::PackageSpec};
use wasm_pkg_loader::Client;
use wit_component::DecodedWasm;

mod oci;
mod warg;

use oci::{GetArgs as OciGetArgs, PushArgs as OciPushArgs};
use warg::{GetArgs as WargGetArgs, PushArgs as WargPushArgs};

#[derive(Parser, Debug)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Args, Debug)]
struct RegistryArgs {
    /// The registry domain to use. Overrides configuration file(s).
    #[arg(long = "registry", value_name = "WKG_DOMAIN")]
    domain: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Load a package. This is for use in debugging dependency fetching. For pulling a component, use `wit get`
    Load(LoadArgs),
    /// Get a component from a registry and write it to a file.
    #[clap(subcommand)]
    Get(GetCommand),
    /// Push a component to a registry.
    #[clap(subcommand)]
    Push(PushCommand),
}

#[derive(Args, Debug)]
struct LoadArgs {
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

    /// The package to get, specified as <namespace>:<name> plus optional
    /// @<version>, e.g. "wasi:cli" or "wasi:http@0.2.0".
    package_spec: PackageSpec,

    #[command(flatten)]
    registry: RegistryArgs,
}

#[derive(ValueEnum, Clone, Debug, PartialEq)]
enum Format {
    Auto,
    Wasm,
    Wit,
}

#[derive(Subcommand, Debug)]
enum GetCommand {
    /// Get a component from an OCI registry and write it to a file.
    Oci(OciGetArgs),
    /// Get a component from a warg registry and write it to a file.
    Warg(WargGetArgs),
}

#[derive(Subcommand, Debug)]
enum PushCommand {
    /// Push a component to an OCI registry.
    Oci(OciPushArgs),
    /// Push a component to a warg registry.
    Warg(WargPushArgs),
}

impl LoadArgs {
    pub async fn run(self) -> anyhow::Result<()> {
        let PackageSpec { package, version } = self.package_spec;

        let mut client = {
            let mut config = Config::default();
            config.set_default_registry(
                "bytecodealliance.org"
                    .parse()
                    .expect("Should be able to parse default registry. This is programmer error"),
            );
            if let Some(file_config) = Config::read_global_config()? {
                config.merge(file_config);
            }
            if let Some(registry) = self.registry.domain {
                let namespace = package.namespace().to_string();
                tracing::debug!(namespace, registry, "overriding namespace registry");
                config.set_namespace_registry(namespace.parse()?, registry.parse()?);
            }
            Client::new(config)
        };

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
            tempfile::NamedTempFile::with_prefix_in(".wkg-load", parent_dir)?.into_parts();
        tracing::debug!(?tmp_path, "Created temporary file");

        let mut content_stream = client.stream_content(&package, &release).await?;

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
                    Some(wit_component::WitPrinter::default().print(&resolve, pkg)?)
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
        Commands::Load(args) => args.run().await,
        Commands::Get(GetCommand::Oci(args)) => args.run().await,
        Commands::Get(GetCommand::Warg(args)) => args.run().await,
        Commands::Push(PushCommand::Oci(args)) => args.run().await,
        Commands::Push(PushCommand::Warg(args)) => args.run().await,
    }
}
