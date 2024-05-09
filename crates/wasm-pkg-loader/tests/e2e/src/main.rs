use std::{
    future::Future,
    io::Write,
    process::{Command, Output},
};

use futures_util::TryStreamExt;
use libtest_mimic::{Arguments, Failed, Trial};
use wasm_pkg_loader::{oci_client, Client, ClientConfig};

const WASM_LAYER_MEDIA_TYPE: &str = "application/vnd.wasm.content.layer.v1+wasm";

macro_rules! tests {
    [$($name:ident),+] => { vec![$(Trial::test(stringify!($name), || run_test($name))),+] };
}

fn main() -> anyhow::Result<()> {
    let args = Arguments::from_args();
    prepare_fixtures()?;
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
const FIXTURE_CONTENT: &[u8] = b"test content";

async fn fetch_smoke_test() {
    // Fetch package
    let mut client_config = ClientConfig::default();
    client_config
        .set_default_registry("localhost:5000")
        .set_oci_registry_config(
            "localhost:5000",
            Some(oci_client::ClientConfig {
                protocol: oci_client::ClientProtocol::Http,
                ..Default::default()
            }),
            None,
        )
        .unwrap();
    let mut client = Client::new(client_config);

    let package = FIXTURE_PACKAGE.parse().unwrap();
    let versions = client.list_all_versions(&package).await.unwrap();
    let version = versions.into_iter().next().unwrap();
    assert_eq!(version.to_string(), FIXTURE_VERSION);

    let release = client.get_release(&package, &version).await.unwrap();
    let content = client
        .stream_content(&package, &release)
        .await
        .unwrap()
        .try_collect::<bytes::BytesMut>()
        .await
        .unwrap();
    assert_eq!(content, FIXTURE_CONTENT);
}

fn prepare_fixtures() -> anyhow::Result<()> {
    // Write content
    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile should work");
    tmp.write_all(FIXTURE_CONTENT)
        .expect("should be able to write to tempfile");
    let tmp_path = tmp.path().to_str().expect("tempfile path should be utf8");

    // Push package with `oras`
    let pkg = FIXTURE_PACKAGE.replace(':', "/");
    let mut cmd = Command::new("oras");
    let output = cmd
        .arg("push")
        .arg(format!("localhost:5000/{pkg}:{FIXTURE_VERSION}",))
        .arg(format!("{tmp_path}:{WASM_LAYER_MEDIA_TYPE}"))
        // Suppress error from absolute tmp_path
        .arg("--disable-path-validation")
        .output();

    if output.as_ref().is_ok_and(|output| output.status.success()) {
        return Ok(());
    }

    match output {
        Ok(Output {
            status,
            stdout,
            stderr,
        }) => {
            eprintln!("Command [{cmd:?}] returned {status}",);
            if !stdout.is_empty() {
                eprintln!("Command stdout:\n{}", String::from_utf8_lossy(&stdout));
            }
            if !stderr.is_empty() {
                eprintln!("Command stderr:\n{}", String::from_utf8_lossy(&stderr));
            }
            eprintln!("\nNOTE: These tests expect an OCI distribution server to be running at localhost:5000.\n");
        }
        Err(err) => {
            eprintln!("Command [{cmd:?}] failed to execute: {err:?}");
            eprintln!("\nNOTE: These tests expect the `oras` command to be available in PATH.\n");
        }
    }
    Err(anyhow::anyhow!("Fixture package creation failed."))
}
