use std::{collections::HashMap, path::Path};

use rstest::rstest;
use tokio::process::Command;
use wasm_pkg_core::{
    config::{Config, Override},
    lock::LockFile,
    wit::{self, OutputType},
};

mod common;

#[rstest]
#[case("dog-fetcher", 1)]
#[case("cli-example", 2)]
#[tokio::test]
async fn test_fetch(
    #[case] fixture_name: &str,
    #[case] expected_deps: usize,
    #[values(OutputType::Wasm, OutputType::Wit)] output: OutputType,
) {
    let (_temp, fixture_path) = common::load_fixture(fixture_name).await.unwrap();
    let lock_file = fixture_path.join("wkg.lock");
    let mut lock = LockFile::new_with_path([], &lock_file)
        .await
        .expect("Should be able to create a new lock file");
    let (_temp_cache, client) = common::get_client().await.unwrap();

    wit::fetch_dependencies(
        &Config::default(),
        fixture_path.join("wit"),
        &mut lock,
        client,
        output,
    )
    .await
    .expect("Should be able to fetch the dependencies");

    assert_eq!(
        lock.packages.len(),
        expected_deps,
        "Should have the correct number of packages in the lock file"
    );

    // Now try to build the component to make sure the deps work
    build_component(&fixture_path).await;
}

#[rstest]
#[tokio::test]
async fn test_nested_local(#[values(OutputType::Wasm, OutputType::Wit)] output: OutputType) {
    let (_temp, fixture_path) = common::load_fixture("nested-local").await.unwrap();
    let project_path = fixture_path.join("project");
    let lock_file = project_path.join("wkg.lock");
    let mut lock = LockFile::new_with_path([], &lock_file)
        .await
        .expect("Should be able to create a new lock file");
    let mut config = Config::default();
    let overrides = config.overrides.get_or_insert(HashMap::default());
    overrides.insert(
        "my:local".to_string(),
        Override {
            path: Some(fixture_path.join("local-dep").join("wit")),
            ..Default::default()
        },
    );
    let (_temp_cache, client) = common::get_client().await.unwrap();

    wit::fetch_dependencies(&config, project_path.join("wit"), &mut lock, client, output)
        .await
        .expect("Should be able to fetch the dependencies");

    assert_eq!(
        lock.packages.len(),
        1,
        "Should have the correct number of packages in the lock file"
    );
}

async fn build_component(fixture_path: &Path) {
    let output = Command::new(env!("CARGO"))
        .current_dir(fixture_path)
        .arg("build")
        .output()
        .await
        .expect("Should be able to execute build command");
    assert!(output.status.success(), "Should be able to build the component successfully. Exited with error code: {}\nStdout:\n\n{}\n\nStderr:\n\n{}", output.status.code().unwrap_or(-1), String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}
