use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddrV4},
    path::{Path, PathBuf},
};

use oci_client::client::ClientConfig;
use testcontainers::{
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
    ContainerAsync, GenericImage, ImageExt,
};
use tokio::{net::TcpListener, process::Command};
use wasm_pkg_client::{oci::OciRegistryConfig, Config, CustomConfig, Registry, RegistryMetadata};

/// Returns an open port on localhost
pub async fn find_open_port() -> u16 {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("failed to bind random port")
        .local_addr()
        .map(|addr| addr.port())
        .expect("failed to get local address from opened TCP socket")
}

/// Starts a registry container on an open port, returning a [`Config`] with registry auth
/// configured for it and the name of the registry. The container handle is also returned as it must
/// be kept in scope
pub async fn start_registry() -> (Config, Registry, ContainerAsync<GenericImage>) {
    let port = find_open_port().await;
    let container = GenericImage::new("registry", "2")
        .with_wait_for(WaitFor::message_on_stderr("listening on [::]:5000"))
        .with_mapped_port(port, 5000.tcp())
        .start()
        .await
        .expect("Failed to start test container");

    let registry: Registry = format!("localhost:{}", port).parse().unwrap();
    let mut config = Config::empty();
    // Make sure we add wasi. The default fallbacks don't get written to disk, which we use in
    // tests, so we just start with an empty config and write this into it
    config.set_namespace_registry(
        "wasi".parse().unwrap(),
        wasm_pkg_client::RegistryMapping::Registry("wasi.dev".parse().unwrap()),
    );
    let reg_conf = config.get_or_insert_registry_config_mut(&registry);
    reg_conf
        .set_backend_config(
            "oci",
            OciRegistryConfig {
                client_config: ClientConfig {
                    protocol: oci_client::client::ClientProtocol::Http,
                    ..Default::default()
                },
                credentials: None,
            },
        )
        .unwrap();

    (config, registry, container)
}

/// Clones the given config, mapping the namespace to the given registry at the top level
pub fn map_namespace(config: &Config, namespace: &str, registry: &Registry) -> Config {
    let mut config = config.clone();
    let mut metadata = RegistryMetadata::default();
    metadata.preferred_protocol = Some("oci".to_string());
    let mut meta = serde_json::Map::new();
    meta.insert("registry".to_string(), registry.to_string().into());
    metadata.protocol_configs = HashMap::from_iter([("oci".to_string(), meta)]);
    config.set_namespace_registry(
        namespace.parse().unwrap(),
        wasm_pkg_client::RegistryMapping::Custom(CustomConfig {
            registry: registry.to_owned(),
            metadata,
        }),
    );
    config
}

/// A loaded fixture with helpers for running wkg tests
pub struct Fixture {
    pub temp_dir: tempfile::TempDir,
    pub fixture_path: PathBuf,
}

impl Fixture {
    /// Returns a base `wkg` command for running tests with the current directory set to the loaded
    /// fixture and with a separate wkg cache dir
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_wkg"));
        cmd.current_dir(&self.fixture_path);
        cmd.env("WKG_CACHE_DIR", self.temp_dir.path().join("cache"));
        cmd
    }

    /// Same as [`Fixture::command`] but also writes the given config to disk and sets the
    /// `WKG_CONFIG` environment variable to the path of the config
    pub async fn command_with_config(&self, config: &Config) -> Command {
        let config_path = self.temp_dir.path().join("config.toml");
        config
            .to_file(&config_path)
            .await
            .expect("failed to write config");
        let mut cmd = self.command();
        cmd.env("WKG_CONFIG_FILE", config_path);
        cmd
    }
}

/// Gets the path to the fixture
pub fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Loads the fixture with the given name into a temporary directory. This will copy the fixture
/// from the tests/fixtures directory into a temporary directory and return the tempdir containing
/// that directory (and its path)
pub async fn load_fixture(fixture: &str) -> Fixture {
    let temp_dir = tempfile::tempdir().expect("Failed to create tempdir");
    let fixture_path = fixture_dir().join(fixture);
    // This will error if it doesn't exist, which is what we want
    tokio::fs::metadata(&fixture_path)
        .await
        .expect("Fixture does not exist or couldn't be read");
    let copied_path = temp_dir.path().join(fixture_path.file_name().unwrap());
    copy_dir(&fixture_path, &copied_path)
        .await
        .expect("Failed to copy fixture");
    Fixture {
        temp_dir,
        fixture_path: copied_path,
    }
}

#[allow(dead_code)]
async fn copy_dir(source: impl AsRef<Path>, destination: impl AsRef<Path>) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(&destination).await?;
    let mut entries = tokio::fs::read_dir(source).await?;
    while let Some(entry) = entries.next_entry().await? {
        let filetype = entry.file_type().await?;
        if filetype.is_dir() {
            // Skip the deps directory in case it is there from debugging
            if entry.path().file_name().unwrap_or_default() == "deps" {
                continue;
            }
            Box::pin(copy_dir(
                entry.path(),
                destination.as_ref().join(entry.file_name()),
            ))
            .await?;
        } else {
            let path = entry.path();
            let extension = path.extension().unwrap_or_default();
            // Skip any .lock files that might be there from debugging
            if extension == "lock" {
                continue;
            }
            tokio::fs::copy(path, destination.as_ref().join(entry.file_name())).await?;
        }
    }
    Ok(())
}
