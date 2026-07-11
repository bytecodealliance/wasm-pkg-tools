use wasm_pkg_client::{Version, VersionInfo};

use crate::common::{load_fixture_from, transitive_local_fixture};
#[cfg(feature = "docker-tests")]
use crate::common::{map_transitive_local_namespaces, publish_transitive_local};

mod common;

#[cfg(feature = "docker-tests")]
#[tokio::test]
async fn build_and_publish_with_metadata() {
    use oci_client::{Reference, client::ClientConfig, manifest::OciManifest};
    use wasm_pkg_core::manifest::{MANIFEST_FILE_NAME, Manifest};

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
        panic!("OciManifest should be an image manifest");
    };

    let annotations = manifest
        .annotations
        .expect("OciManifest should have annotations");

    let manifest = Manifest::load_from_path(fixture.fixture_path.join(MANIFEST_FILE_NAME))
        .await
        .expect("Should be able to load wkg manifest");
    let meta = manifest.metadata.expect("Should have metadata");

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

#[cfg(feature = "docker-tests")]
#[tokio::test]
async fn publish_workspace_packages() {
    let (config, registry, _container) = common::start_registry().await;
    let namespaces = ["example-a", "example-b", "example-c", "example-d"];

    let fixture = load_fixture_from(transitive_local_fixture()).await;

    assert!(
        fixture.fixture_path.join("wkg.toml").exists(),
        "fixture must include the workspace manifest copied from \
         crates/wasm-pkg-core/tests/fixtures/transitive-local/wkg.toml",
    );

    let mut mapped = config.clone();
    for ns in namespaces {
        mapped = common::map_namespace(&mapped, ns, &registry);
    }

    let status = fixture
        .command_with_config(&mapped)
        .await
        .args(["publish", "--workspace"])
        .status()
        .await
        .expect("spawn wkg publish");
    assert!(status.success(), "`wkg publish --workspace` should succeed",);

    let client = wasm_pkg_client::Client::new(mapped);
    let expected_version = "0.1.0".parse::<Version>().unwrap();
    for name in [
        "example-a:foo",
        "example-b:bar",
        "example-c:baz",
        "example-c:nested",
        "example-d:foo",
    ] {
        let pkg = name.parse().unwrap();
        let versions = client
            .list_all_versions(&pkg)
            .await
            .unwrap_or_else(|e| panic!("list versions for {name}: {e:#}"));
        std::assert_matches!(
            &versions[..],
            [VersionInfo { version, .. }] if version == &expected_version,
            "{name} should have exactly one published version",
        );
    }
}

#[tokio::test]
async fn fetch_workspace_packages() {
    let fixture = load_fixture_from(transitive_local_fixture()).await;

    let status = fixture
        .command_with_config(&wasm_pkg_client::Config::empty())
        .await
        .arg("fetch")
        .status()
        .await
        .expect("spawn wkg fetch");
    assert!(status.success(), "`wkg fetch` in workspace should succeed",);

    assert!(
        fixture.fixture_path.join("wkg.lock").exists(),
        "workspace fetch should create wkg.lock at the workspace root",
    );
    let aggregated_deps = fixture.fixture_path.join("wkg/deps");
    assert!(
        aggregated_deps.is_dir(),
        "workspace fetch should create <root>/wkg/deps/",
    );

    let mut entries = tokio::fs::read_dir(&aggregated_deps).await.unwrap();
    assert!(
        entries.next_entry().await.unwrap().is_none(),
        "aggregated deps should be empty when every dep is a workspace member",
    );
}

#[tokio::test]
pub async fn check() {
    // Use an explicit config that maps `wasi` to `wasi.dev`.
    let mut config = wasm_pkg_client::Config::empty();
    config.set_namespace_registry(
        "wasi".parse().unwrap(),
        wasm_pkg_client::RegistryMapping::Registry("wasi.dev".parse().unwrap()),
    );

    let fixture = common::load_fixture("wasi-http").await;
    let output = fixture.temp_dir.path().join("out");

    let get = fixture
        .command_with_config(&config)
        .await
        .arg("get")
        .arg("wasi:http")
        .arg("--output")
        .arg(&output)
        .status()
        .await
        .unwrap();
    assert!(get.success());

    let check_same = fixture
        .command_with_config(&config)
        .await
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
        .command_with_config(&config)
        .await
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
