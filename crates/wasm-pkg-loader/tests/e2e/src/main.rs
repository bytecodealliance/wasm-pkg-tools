use std::future::Future;

use anyhow::Context;
use futures_util::TryStreamExt;
use libtest_mimic::{Arguments, Failed, Trial};
use oci_wasm::{WasmClient, WasmConfig};
use wasm_pkg_common::{
    config::{oci::OciRegistryConfig, Config},
    Registry,
};
use wasm_pkg_loader::{oci_client, Client};

macro_rules! tests {
    [$($name:ident),+] => { vec![$(Trial::test(stringify!($name), || run_test($name))),+] };
}

fn main() -> anyhow::Result<()> {
    let args = Arguments::from_args();
    tokio_test::block_on(prepare_fixtures())?;
    let tests = tests![fetch_smoke_test];
    libtest_mimic::run(&args, tests).exit();
}

fn run_test<F, Fut>(f: F) -> Result<(), Failed>
where
    F: Fn() -> Fut,
    Fut: Future,
{
    tokio_test::block_on(f());
    Ok(())
}

const FIXTURE_PACKAGE: &str = "test:pkg";
const FIXTURE_VERSION: &str = "1.0.0";
const FIXTURE_WASM: &str = "./testdata/binary_wit.wasm";

async fn fetch_smoke_test() {
    // Fetch package
    let mut client_config = Config::default();
    let registry: Registry = "localhost:5001".parse().unwrap();
    client_config.set_default_registry(registry.clone());
    let reg_config = client_config.get_or_insert_registry_config_mut(&registry);
    reg_config
        .set_backend_config(
            "oci".to_string(),
            OciRegistryConfig {
                auth: None,
                protocol: Some(oci_client::ClientProtocol::Http),
            },
        )
        .expect("Should be able to set config");
    reg_config.set_backend_type("oci".to_string());

    let mut client = Client::new(client_config);

    let package = FIXTURE_PACKAGE.parse().unwrap();
    let versions = client.list_all_versions(&package).await.unwrap();
    let version = versions.into_iter().next().unwrap();
    assert_eq!(version.to_string(), FIXTURE_VERSION);

    let release = client
        .get_release(&package, &version.version)
        .await
        .unwrap();
    let content = client
        .stream_content(&package, &release)
        .await
        .unwrap()
        .try_collect::<bytes::BytesMut>()
        .await
        .unwrap();
    let expected_content = tokio::fs::read(FIXTURE_WASM)
        .await
        .expect("Failed to read fixture");
    assert_eq!(content, expected_content);
}

fn get_client() -> WasmClient {
    let client = oci_distribution::Client::new(oci_distribution::client::ClientConfig {
        protocol: oci_client::ClientProtocol::HttpsExcept(vec!["localhost:5001".to_string()]),
        ..Default::default()
    });
    WasmClient::new(client)
}

async fn prepare_fixtures() -> anyhow::Result<()> {
    let pkg = FIXTURE_PACKAGE.replace(':', "/");
    let client = get_client();

    let image =
        oci_distribution::Reference::try_from(format!("localhost:5001/{pkg}:{FIXTURE_VERSION}"))
            .unwrap();

    let (conf, component) = WasmConfig::from_component(FIXTURE_WASM, None)
        .await
        .context("Should be able to parse component and create config")?;
    client
        .push(
            &image,
            &oci_distribution::secrets::RegistryAuth::Anonymous,
            component,
            conf,
            None,
        )
        .await
        .context("Should be able to push component")?;
    Ok(())
}
