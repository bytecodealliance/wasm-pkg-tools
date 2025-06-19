use futures_util::TryStreamExt;
use wasm_pkg_client::{Client, Config};

const FIXTURE_WASM: &str = "./tests/testdata/binary_wit.wasm";

#[cfg(feature = "docker-tests")]
#[tokio::test]
async fn publish_and_fetch_smoke_test() {
    use testcontainers::{
        core::{IntoContainerPort, WaitFor},
        runners::AsyncRunner,
        GenericImage, ImageExt,
    };

    let _container = GenericImage::new("registry", "2")
        .with_wait_for(WaitFor::message_on_stderr("listening on [::]:5000"))
        .with_mapped_port(5001, 5000.tcp())
        .start()
        .await
        .expect("Failed to start test container");
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

    let (package, _version) = client
        .publish_release_file(FIXTURE_WASM, Default::default())
        .await
        .expect("Failed to publish file");

    let versions = client.list_all_versions(&package).await.unwrap();
    let version = versions.into_iter().next().unwrap();
    assert_eq!(version.to_string(), "0.2.0");

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

// Simple smoke test to make sure the custom metadata section is parsed and used correctly. Down the
// line we might want to just push a thing to a local registry and then fetch it, but for now we'll
// just use the bytecodealliance registry.
#[tokio::test]
async fn fetch_with_custom_config() {
    let toml_config = toml::toml! {
        [namespace_registries]
        wasi = { registry = "fake.com:1234", metadata = { preferredProtocol = "oci", "oci" = {registry = "ghcr.io", namespacePrefix = "bytecodealliance/wasm-pkg/" } } }
    };

    let conf = Config::from_toml(&toml_config.to_string()).expect("Failed to parse config");
    let client = Client::new(conf);

    // Try listing all versions of the wasi package and make sure it doesn't fail
    let package = "wasi:http".parse().unwrap();
    let versions = client
        .list_all_versions(&package)
        .await
        .expect("Should be able to list versions with custom config");
    assert!(
        !versions.is_empty(),
        "Should be able to list versions with custom config"
    );
}
