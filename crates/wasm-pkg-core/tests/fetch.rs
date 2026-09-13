use std::{collections::HashMap, path::Path};

use rstest::rstest;
use tokio::process::Command;
use wasm_pkg_core::{
    lock::LockFile,
    manifest::{Manifest, Override},
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
        &Manifest::default(),
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
    let mut manifest = Manifest::default();
    let overrides = manifest.overrides.get_or_insert(HashMap::default());
    overrides.insert(
        "my:local".to_string(),
        Override {
            path: Some(fixture_path.join("local-dep/wit")),
            ..Default::default()
        },
    );
    let (_temp_cache, client) = common::get_client().await.unwrap();

    wit::fetch_dependencies(
        &manifest,
        project_path.join("wit"),
        &mut lock,
        client,
        output,
    )
    .await
    .expect("Should be able to fetch the dependencies");

    assert_eq!(
        lock.packages.len(),
        1,
        "Should have the correct number of packages in the lock file"
    );
}

#[rstest]
#[tokio::test]
async fn test_transitive_local(#[values(OutputType::Wasm, OutputType::Wit)] output: OutputType) {
    let (_temp, fixture_path) = common::load_fixture("transitive-local").await.unwrap();
    let project_path = fixture_path.join("example-a");
    let lock_file = project_path.join("wkg.lock");
    let mut lock = LockFile::new_with_path([], &lock_file)
        .await
        .expect("Should be able to create a new lock file");
    // Override paths resolve against the working directory, so they must be absolute here
    let manifest = Manifest::from_toml(&format!(
        r#"
[overrides]
"example-b:bar" = {{ path = '{root}/example-b/wit' }}
"example-c:baz" = {{ path = '{root}/example-c/wit' }}
"example-c:nested" = {{ path = '{root}/example-c/wit/nested' }}
"#,
        root = fixture_path.display()
    ))
    .expect("manifest should parse");
    let (_temp_cache, client) = common::get_client().await.unwrap();

    // If overrides didn't properly resolve, this will fail
    wit::fetch_dependencies(
        &manifest,
        project_path.join("wit"),
        &mut lock,
        client,
        output,
    )
    .await
    .unwrap_or_else(|e| panic!("Should be able to fetch the dependencies: {e:#}"));

    // Ensure that the deps directory contains the correct dependencies
    let mut deps_dir = tokio::fs::read_dir(project_path.join("wit/deps"))
        .await
        .expect("Should be able to read the deps directory");
    let mut deps = Vec::new();
    while let Ok(Some(entry)) = deps_dir.next_entry().await {
        deps.push(entry.file_name().to_string_lossy().to_string());
    }
    assert_eq!(deps.len(), 3);
    assert!(deps.contains(&"example-b-bar-0.1.0".to_string()));
    assert!(deps.contains(&"example-c-baz-0.1.0".to_string()));
    assert!(deps.contains(&"example-c-nested-0.1.0".to_string()));

    // All dependencies are local, so the lock file should be empty
    assert_eq!(
        lock.packages.len(),
        0,
        "Should have the correct number of packages in the lock file"
    );
}

/// A world can name several versions of the same package, so an override key may carry an exact
/// version to scope it to just one of them.
#[rstest]
#[tokio::test]
async fn test_multi_version_local(#[values(OutputType::Wasm, OutputType::Wit)] output: OutputType) {
    let (_temp, fixture_path) = common::load_fixture("multi-version-local").await.unwrap();
    let project_path = fixture_path.join("project");
    let lock_file = project_path.join("wkg.lock");
    let mut lock = LockFile::new_with_path([], &lock_file)
        .await
        .expect("Should be able to create a new lock file");
    let manifest = Manifest::from_toml(&format!(
        r#"
[overrides]
"my:local@0.1.0" = {{ path = '{root}/local-dep-0.1.0/wit' }}
"my:local@0.2.0" = {{ path = '{root}/local-dep-0.2.0/wit' }}
"#,
        root = fixture_path.display()
    ))
    .expect("manifest should parse");
    let (_temp_cache, client) = common::get_client().await.unwrap();

    wit::fetch_dependencies(
        &manifest,
        project_path.join("wit"),
        &mut lock,
        client,
        output,
    )
    .await
    .unwrap_or_else(|e| panic!("Should be able to fetch the dependencies: {e:#}"));

    // Both versions must be written out. Before per-version override keys, the two entries
    // collided on the package name and only one of them (nondeterministically) survived.
    let mut deps_dir = tokio::fs::read_dir(project_path.join("wit/deps"))
        .await
        .expect("Should be able to read the deps directory");
    let mut deps = Vec::new();
    while let Ok(Some(entry)) = deps_dir.next_entry().await {
        deps.push(entry.file_name().to_string_lossy().to_string());
    }
    // Local directory deps are written out as directories under both output types
    assert_eq!(deps.len(), 2, "expected both versions, got {deps:?}");
    assert!(
        deps.contains(&"my-local-0.1.0".to_string()),
        "missing my-local-0.1.0 in {deps:?}"
    );
    assert!(
        deps.contains(&"my-local-0.2.0".to_string()),
        "missing my-local-0.2.0 in {deps:?}"
    );

    // each directory must hold its own version's WIT, not two copies of whichever one won
    let v1 = read_dep_wit(&project_path.join("wit/deps/my-local-0.1.0")).await;
    let v2 = read_dep_wit(&project_path.join("wit/deps/my-local-0.2.0")).await;
    assert!(v1.contains("foo: func() -> string"), "0.1.0 body was: {v1}");
    assert!(
        v2.contains("foo: func() -> result<string>"),
        "0.2.0 body was: {v2}"
    );

    // All dependencies are local, so the lock file should be empty
    assert_eq!(
        lock.packages.len(),
        0,
        "Should have the correct number of packages in the lock file"
    );
}

/// The same applies to packages coming from a registry rather than from an override: both
/// versions must be fetched and both must be recorded against the one package in the lock file.
#[rstest]
#[tokio::test]
async fn test_multi_version_registry(
    #[values(OutputType::Wasm, OutputType::Wit)] output: OutputType,
) {
    let (_temp, fixture_path) = common::load_fixture("multi-version-registry")
        .await
        .unwrap();
    let lock_file = fixture_path.join("wkg.lock");
    let mut lock = LockFile::new_with_path([], &lock_file)
        .await
        .expect("Should be able to create a new lock file");
    let (_temp_cache, client) = common::get_client().await.unwrap();

    wit::fetch_dependencies(
        &Manifest::default(),
        fixture_path.join("wit"),
        &mut lock,
        client,
        output,
    )
    .await
    .unwrap_or_else(|e| panic!("Should be able to fetch the dependencies: {e:#}"));

    let mut deps_dir = tokio::fs::read_dir(fixture_path.join("wit/deps"))
        .await
        .expect("Should be able to read the deps directory");
    let mut deps = Vec::new();
    while let Ok(Some(entry)) = deps_dir.next_entry().await {
        deps.push(entry.file_name().to_string_lossy().to_string());
    }
    let suffix = match output {
        OutputType::Wit => "",
        OutputType::Wasm => ".wasm",
    };
    for version in ["0.2.0", "0.2.1"] {
        let expected = format!("wasi-clocks-{version}{suffix}");
        assert!(deps.contains(&expected), "missing {expected} in {deps:?}");
    }

    // Both requirements belong to the one `wasi:clocks` package, so the lock file records a single
    // package holding two locked versions.
    let clocks = lock
        .packages
        .iter()
        .find(|p| p.name.to_string() == "wasi:clocks")
        .expect("wasi:clocks should be in the lock file");
    let mut locked: Vec<String> = clocks
        .versions
        .iter()
        .map(|v| v.version.to_string())
        .collect();
    locked.sort();
    assert_eq!(locked, ["0.2.0", "0.2.1"], "lock file versions: {locked:?}");
}

/// A bare key already covers every version, so pairing it with a versioned key for the same
/// package is ambiguous and resolving must reject it rather than pick a winner by map ordering.
#[tokio::test]
async fn test_conflicting_override_keys_rejected() {
    let (_temp, fixture_path) = common::load_fixture("multi-version-local").await.unwrap();
    let project_path = fixture_path.join("project");
    let mut lock = LockFile::new_with_path([], project_path.join("wkg.lock"))
        .await
        .expect("Should be able to create a new lock file");
    // Built directly rather than parsed from TOML, so `Manifest::validate` never ran
    let manifest = Manifest {
        overrides: Some(HashMap::from([
            (
                "my:local".to_string(),
                Override {
                    path: Some(fixture_path.join("local-dep-0.1.0/wit")),
                    version: None,
                },
            ),
            (
                "my:local@0.2.0".to_string(),
                Override {
                    path: Some(fixture_path.join("local-dep-0.2.0/wit")),
                    version: None,
                },
            ),
        ])),
        ..Default::default()
    };
    let (_temp_cache, client) = common::get_client().await.unwrap();

    let err = wit::fetch_dependencies(
        &manifest,
        project_path.join("wit"),
        &mut lock,
        client,
        OutputType::Wit,
    )
    .await
    .expect_err("conflicting override keys should be rejected");

    let err = format!("{err:#}");
    assert!(
        err.contains("my:local@0.2.0") && err.contains("overlap"),
        "unexpected error: {err}"
    );
}

/// A versioned override key must name the package actually at its path; a mismatch is an error
/// naming both the key and what is on disk.
#[tokio::test]
async fn test_override_key_must_match_package_on_disk() {
    let (_temp, fixture_path) = common::load_fixture("multi-version-local").await.unwrap();
    let project_path = fixture_path.join("project");
    let mut lock = LockFile::new_with_path([], project_path.join("wkg.lock"))
        .await
        .expect("Should be able to create a new lock file");
    let manifest = Manifest::from_toml(&format!(
        r#"
[overrides]
"my:local@0.1.0" = {{ path = '{root}/local-dep-0.2.0/wit' }}
"#,
        root = fixture_path.display()
    ))
    .expect("manifest should parse");
    let (_temp_cache, client) = common::get_client().await.unwrap();

    let err = wit::fetch_dependencies(
        &manifest,
        project_path.join("wit"),
        &mut lock,
        client,
        OutputType::Wit,
    )
    .await
    .expect_err("an override key that does not match the package on disk should be rejected");

    let err = format!("{err:#}");
    assert!(
        err.contains("override `my:local@0.1.0` points at")
            && err.contains("is package `my:local@0.2.0`"),
        "unexpected error: {err}"
    );
}

/// Concatenates every WIT file in a dep directory; the file names inside differ by output type.
async fn read_dep_wit(dir: &Path) -> String {
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .unwrap_or_else(|e| panic!("Should be able to read {}: {e}", dir.display()));
    let mut contents = String::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        contents.push_str(&tokio::fs::read_to_string(entry.path()).await.unwrap());
    }
    contents
}

async fn build_component(fixture_path: &Path) {
    let output = Command::new(env!("CARGO"))
        .current_dir(fixture_path)
        .arg("build")
        .output()
        .await
        .expect("Should be able to execute build command");
    assert!(
        output.status.success(),
        "Should be able to build the component successfully. Exited with error code: {}\nStdout:\n\n{}\n\nStderr:\n\n{}",
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
