use std::future::Future;

use anyhow::Context;
use futures_util::TryStreamExt;
use libtest_mimic::{Arguments, Failed, Trial};
use oci_wasm::{WasmClient, WasmConfig};
use wasm_pkg_client::{Client, Config};

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
    let config = Config::from_toml(
        r#"
        default_registry = "localhost:5001"

        [registry."localhost:5001"]
        type = "oci"
        [registry."localhost:5001".oci]
        protocol = "http"
    "#,
    )
    .unwrap();
    let client = Client::new(config);

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
    let client = oci_client::Client::new(oci_client::client::ClientConfig {
        protocol: oci_client::client::ClientProtocol::HttpsExcept(vec![
            "localhost:5001".to_string()
        ]),
        ..Default::default()
    });
    WasmClient::new(client)
}

async fn prepare_fixtures() -> anyhow::Result<()> {
    let pkg = FIXTURE_PACKAGE.replace(':', "/");
    let client = get_client();

    let image =
        oci_client::Reference::try_from(format!("localhost:5001/{pkg}:{FIXTURE_VERSION}")).unwrap();

    let (conf, component) = WasmConfig::from_component(FIXTURE_WASM, None)
        .await
        .context("Should be able to parse component and create config")?;
    client
        .push(
            &image,
            &oci_client::secrets::RegistryAuth::Anonymous,
            component,
            conf,
            None,
        )
        .await
        .context("Should be able to push component")?;
    Ok(())
}
