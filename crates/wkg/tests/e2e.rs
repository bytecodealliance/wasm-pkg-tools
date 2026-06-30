use wasm_pkg_client::{Version, VersionInfo};

use crate::common::copy_dir;

mod common;

#[cfg(feature = "docker-tests")]
#[tokio::test]
async fn build_and_publish_with_metadata() {
    use oci_client::{client::ClientConfig, manifest::OciManifest, Reference};
    use wasm_pkg_core::manifest::{Manifest, MANIFEST_FILE_NAME};

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
async fn publish_multiple_transitive_local_packages() {
    use std::path::PathBuf;

    let (config, registry, _container) = common::start_registry().await;
    let namespaces = ["example-a", "example-b", "example-c", "example-d"];

    // copy the transitive-local fixtures from wasm-pkg-core into a temp dir
    let temp_dir = tempfile::tempdir().expect("Failed to create tempdir");
    let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../wasm-pkg-core/tests/fixtures/transitive-local");
    let fixture_root = temp_dir.path().join("transitive-local");
    copy_dir(&src_root, &fixture_root).await.unwrap();

    let mut mapped = config.clone();
    for ns in namespaces {
        mapped = common::map_namespace(&mapped, ns, &registry);
    }
    let config_path = temp_dir.path().join("config.toml");
    mapped.to_file(&config_path).await.expect("write config");

    // pass all fixture dirs to a single `wkg publish` invocation
    let mut dirs: Vec<PathBuf> = namespaces
        .iter()
        .map(|name| fixture_root.join(name).join("wit"))
        .collect();
    // TODO use glob suchas in `wasm_pkgs_core::resolver::tests::transitive_local_paths`
    dirs.push(fixture_root.join("example-c/wit/nested"));

    let mut publish = tokio::process::Command::new(env!("CARGO_BIN_EXE_wkg"));
    publish
        .current_dir(temp_dir.path())
        .env("WKG_CACHE_DIR", temp_dir.path().join("cache"))
        .env("WKG_CONFIG_FILE", &config_path)
        .arg("publish");
    for dir in &dirs {
        publish.arg(dir);
    }
    let status = publish.status().await.expect("spawn wkg publish");
    assert!(status.success(), "wkg publish should succeed");

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
