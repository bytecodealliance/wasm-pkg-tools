mod common;

#[cfg(feature = "docker-tests")]
#[tokio::test]
async fn build_and_publish_with_metadata() {
    use oci_client::{client::ClientConfig, manifest::OciManifest, Reference};

    let (config, registry, _container) = common::start_registry().await;

    let fixture = common::load_fixture("wasi-http").await;

    let status = fixture
        .command_with_config(&config)
        .await
        .args(["wit", "build"])
        .status()
        .await
        .expect("Should be able to build wit packagee");
    assert!(status.success(), "Build should succeed");

    let namespace_mapped_config = common::map_namespace(&config, "wasi", &registry);
    let status = fixture
        .command_with_config(&namespace_mapped_config)
        .await
        .args(["publish", "wasi:http@0.2.0.wasm"])
        .status()
        .await
        .expect("Should be able to publish wit package");
    assert!(status.success(), "Publish should succeed");

    // Now fetch the manifest and verify the annotations are present
    let client = oci_client::Client::new(ClientConfig {
        protocol: oci_client::client::ClientProtocol::Http,
        ..Default::default()
    });

    let reference: Reference = format!("{registry}/wasi/http:0.2.0").parse().unwrap();
    let (manifest, _) = client
        .pull_manifest(&reference, &oci_client::secrets::RegistryAuth::Anonymous)
        .await
        .expect("Should be able to fetch manifest");

    let manifest = if let OciManifest::Image(m) = manifest {
        m
    } else {
        panic!("Manifest should be an image manifest");
    };

    let annotations = manifest
        .annotations
        .expect("Manifest should have annotations");

    let wkg_toml =
        wasm_pkg_core::config::Config::load_from_path(fixture.fixture_path.join("wkg.toml"))
            .await
            .expect("Should be able to load wkg.toml");
    let meta = wkg_toml.metadata.expect("Should have metadata");

    assert_eq!(
        annotations
            .get("org.opencontainers.image.version")
            .expect("Should have version"),
        "0.2.0",
        "Version should be 0.2.0"
    );
    assert_eq!(
        annotations.get("org.opencontainers.image.description"),
        meta.description.as_ref(),
        "Description should match"
    );
    assert_eq!(
        annotations.get("org.opencontainers.image.licenses"),
        meta.licenses.as_ref(),
        "License should match"
    );
    assert_eq!(
        annotations.get("org.opencontainers.image.source"),
        meta.source.as_ref(),
        "Source should match"
    );
    assert_eq!(
        annotations.get("org.opencontainers.image.url"),
        meta.homepage.as_ref(),
        "Name should match"
    );
}

#[tokio::test]
pub async fn check() {
    let fixture = common::load_fixture("wasi-http").await;
    let output = fixture.temp_dir.path().join("out");

    let get = fixture
        .command()
        .arg("get")
        .arg("wasi:http")
        .arg("--output")
        .arg(&output)
        .status()
        .await
        .unwrap();
    assert!(get.success());

    let check_same = fixture
        .command()
        .arg("get")
        .arg("--check")
        .arg("wasi:http")
        .arg("--output")
        .arg(&output)
        .status()
        .await
        .unwrap();
    assert!(check_same.success());

    std::fs::write(&output, vec![1, 2, 3, 4]).expect("overwrite output with bogus contents");

    let check_diff = fixture
        .command()
        .arg("get")
        .arg("--check")
        .arg("wasi:http")
        .arg("--output")
        .arg(output)
        .status()
        .await
        .unwrap();
    assert!(!check_diff.success());
}
