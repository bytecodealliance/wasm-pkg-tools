use futures_util::TryStreamExt;
use testcontainers::{
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
    GenericImage, ImageExt,
};
use wasm_pkg_client::{Client, Config};

const FIXTURE_WASM: &str = "./tests/testdata/binary_wit.wasm";

#[cfg(any(target_os = "linux", feature = "_local"))]
// NOTE: These are only run on linux for CI purposes, because they rely on the docker client being
// available, and for various reasons this has proven to be problematic on both the Windows and
// MacOS runners due to it not being installed (yay licensing).
#[tokio::test]
async fn publish_and_fetch_smoke_test() {
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
