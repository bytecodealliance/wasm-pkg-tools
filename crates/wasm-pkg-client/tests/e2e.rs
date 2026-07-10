use futures_util::TryStreamExt;
use wasm_pkg_client::{Client, Config, PublishOpts};

const FIXTURE_WASM: &str = "./tests/testdata/binary_wit.wasm";

#[cfg(feature = "docker-tests")]
#[tokio::test]
async fn publish_and_fetch_smoke_test() {
    use testcontainers::{
        GenericImage, ImageExt,
        core::{IntoContainerPort, WaitFor},
        runners::AsyncRunner,
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

// Exercises the publish-time semver gate against a real OCI registry. The
// override package name guarantees the namespace is empty on the registry, so
// `list_matching_versions` must swallow the OCI `NameUnknown` response into an
// empty history for the publish to succeed.
#[cfg(feature = "docker-tests")]
#[tokio::test]
async fn publish_with_semver_check_succeeds_for_new_package() {
    use testcontainers::{
        GenericImage, ImageExt,
        core::{IntoContainerPort, WaitFor},
        runners::AsyncRunner,
    };

    let _container = GenericImage::new("registry", "2")
        .with_wait_for(WaitFor::message_on_stderr("listening on [::]:5000"))
        .with_mapped_port(5002, 5000.tcp())
        .start()
        .await
        .expect("Failed to start test container");

    let config = Config::from_toml(
        r#"
        default_registry = "localhost:5002"

        [registry."localhost:5002"]
        type = "oci"
        [registry."localhost:5002".oci]
        protocol = "http"
    "#,
    )
    .unwrap();
    let client = Client::new(config);

    let package = "example:fresh-series".parse().unwrap();
    let version = "1.0.0".parse().unwrap();

    client
        .publish_release_file(
            FIXTURE_WASM,
            PublishOpts {
                package: Some((package, version)),
                ..Default::default()
            },
        )
        .await
        .expect("publish should succeed for a brand-new package (NameUnknown swallowed)");
}

#[cfg(feature = "docker-tests")]
#[tokio::test]
async fn publish_and_fetch_succeed_with_self_signed_registry() {
    use testcontainers::{
        GenericImage, ImageExt,
        core::{IntoContainerPort, WaitFor},
        runners::AsyncRunner,
    };

    let (fixture_pem_cert, fixture_pem_key) = generate_self_signed_tls_fixture();

    let test_cases = [
        (
            "extra_root_certificates",
            format!(
                r#"
        extra_root_certificates = [
            {{ encoding = "pem", data = """{fixture_pem_cert}""" }}
        ]
    "#
            ),
        ),
        (
            "accept_invalid_certificates",
            "accept_invalid_certificates = true".to_string(),
        ),
    ];

    for (case_name, case_config) in test_cases {
        let container = GenericImage::new("registry", "2")
            .with_wait_for(WaitFor::message_on_stderr("listening on"))
            .with_exposed_port(5000.tcp())
            .with_copy_to("/certs/domain.crt", fixture_pem_cert.as_bytes().to_vec())
            .with_copy_to("/certs/domain.key", fixture_pem_key.as_bytes().to_vec())
            .with_env_var("REGISTRY_HTTP_ADDR", "0.0.0.0:5000")
            .with_env_var("REGISTRY_HTTP_TLS_CERTIFICATE", "/certs/domain.crt")
            .with_env_var("REGISTRY_HTTP_TLS_KEY", "/certs/domain.key")
            .start()
            .await
            .expect("Failed to start TLS registry test container");

        let port = container.get_host_port_ipv4(5000.tcp()).await.unwrap();

        let config = Config::from_toml(&format!(
            r#"
        default_registry = "localhost:{port}"

        [registry."localhost:{port}"]
        type = "oci"
        [registry."localhost:{port}".oci]
        protocol = "https"
        {case_config}
    "#
        ))
        .unwrap();
        let client = Client::new(config);

        let (package, _version) = client
            .publish_release_file(FIXTURE_WASM, Default::default())
            .await
            .unwrap_or_else(|e| panic!("[{case_name}] publish should succeed:\n{e:#}"));

        let release = client
            .get_release(&package, &"0.2.0".parse().unwrap())
            .await
            .unwrap_or_else(|e| panic!("[{case_name}] get_release should succeed:\n{e:#}"));
        if let Err(err) = client.stream_content(&package, &release).await {
            panic!("[{case_name}] fetch should succeed over self-signed TLS:\n{err:#}");
        }
    }
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

fn generate_self_signed_tls_fixture() -> (String, String) {
    let subject_alt_names = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(subject_alt_names)
            .expect("Failed to generate self-signed TLS certificate");
    (cert.pem(), signing_key.serialize_pem())
}
