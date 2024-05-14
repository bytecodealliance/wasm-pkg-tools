use std::path::Path;

use anyhow::{bail, ensure, Context};
use futures_util::TryStreamExt;
use tokio::io::AsyncWriteExt;
use wasm_pkg_loader::{Client, ClientConfig, PackageRef, Release, Version};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut args = std::env::args();
    let arg0 = args.next().unwrap_or_else(|| "wasm-pkg-loader".into());
    let (Some(package), subcmd, version) = (
        args.next(),
        args.next().unwrap_or("show".into()),
        args.next(),
    ) else {
        bail!("usage: {arg0} <package> {{show | fetch}} [version]");
    };

    let client = {
        let mut config = ClientConfig::default();
        config.set_namespace_registry("wasi", "bytecodealliance.org");
        if let Some(file_config) = ClientConfig::from_default_file()? {
            config.merge_config(file_config);
        }

        config.to_client()
    };

    let package: PackageRef = package.parse().context("invalid package ref format")?;

    let version = version
        .map(|ver| ver.parse().context("invalid version format"))
        .transpose()?;

    match subcmd.as_str() {
        "show" => show_package_info(client, package, version).await,
        "fetch" => fetch_package_content(client, package, version).await,
        other => bail!("unknown subcmd {other:?}"),
    }
}

async fn show_package_info(
    mut client: Client,
    package: PackageRef,
    version: Option<Version>,
) -> anyhow::Result<()> {
    if let Some(version) = version {
        let Release {
            version,
            content_digest,
        } = client
            .get_release(&package, &version)
            .await
            .with_context(|| format!("error resolving {package}@{version}"))?;
        println!("Release: {package}@{version}");
        println!("Content digest: {content_digest}");
    } else {
        let mut versions = client
            .list_all_versions(&package)
            .await
            .with_context(|| format!("error listing {package} releases"))?;
        versions.sort();
        println!("Package: {package}");
        println!("Versions:");
        for ver in versions {
            println!(
                "  {ver}{yanked}",
                ver = ver.version,
                yanked = if ver.yanked { " - Yanked" } else { "" }
            );
        }
    }
    Ok(())
}

async fn fetch_package_content(
    mut client: Client,
    package: PackageRef,
    version: Option<Version>,
) -> anyhow::Result<()> {
    let version = match version {
        Some(version) => version,
        None => {
            eprintln!("No version specified; looking up latest release...");
            let versions = client
                .list_all_versions(&package)
                .await
                .with_context(|| format!("error listing {package} releases"))?;
            versions
                .into_iter()
                .map(|v| v.version)
                .max()
                .with_context(|| format!("no releases found for {package}"))?
        }
    };
    eprintln!("Fetching release details for {package}@{version}...");

    let release = client
        .get_release(&package, &version)
        .await
        .context("error getting release details")?;

    let filename = format!(
        "{}-{}-{}.wasm",
        package.namespace(),
        package.name(),
        version
    );
    eprintln!("Downloading content to {filename:?}...");
    ensure!(
        !Path::new(&filename).exists(),
        "{filename:?} already exists"
    );
    let mut content_stream = client.stream_content(&package, &release).await?;

    let mut file = tokio::fs::File::create(filename).await?;
    while let Some(chunk) = content_stream.try_next().await? {
        file.write_all(&chunk).await?;
    }

    Ok(())
}
