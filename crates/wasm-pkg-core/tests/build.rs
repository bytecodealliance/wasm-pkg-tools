use wasm_pkg_core::{config::Config as WkgConfig, lock::LockFile};
use wit_component::DecodedWasm;
use wit_parser::Stability;

mod common;

#[tokio::test]
async fn test_build_wit() {
    let (_temp, fixture_path) = common::load_fixture("wasi-http").await.unwrap();
    let lock_file = fixture_path.join("wkg.lock");

    let mut lock = LockFile::new_with_path([], &lock_file)
        .await
        .expect("Should be able to create a new lock file");
    let (_temp_cache, client) = common::get_client().await.unwrap();
    let (pkg, version, bytes) = wasm_pkg_core::wit::build_package(
        &WkgConfig::default(),
        fixture_path.join("wit"),
        &mut lock,
        client,
    )
    .await
    .expect("Should be able to build the package");

    assert_eq!(
        pkg.to_string(),
        "wasi:http",
        "Should have the correct package reference"
    );
    assert_eq!(
        version.unwrap().to_string(),
        "0.2.3",
        "Should have the correct version"
    );

    // Make sure the lock file has all the correct packages in it
    // NOTE: We could improve this test to check the version too
    let mut names = lock
        .packages
        .iter()
        .map(|p| p.name.to_string())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        vec!["wasi:cli", "wasi:clocks", "wasi:io", "wasi:random"],
        "Should have the correct packages in the lock file"
    );

    // Parse the bytes and make sure it roundtrips back correctly
    let parsed = wit_component::decode(&bytes).expect("Should be able to parse the bytes");
    let (resolve, pkg_id) = match parsed {
        DecodedWasm::WitPackage(resolve, pkg_id) => (resolve, pkg_id),
        _ => panic!("Should be a package"),
    };

    let package = resolve
        .packages
        .get(pkg_id)
        .expect("Should contain decoded package");

    assert_eq!(
        package.name.to_string(),
        "wasi:http@0.2.3",
        "Should have the correct package name"
    );

    // @unstable items are retained
    let types_id = package
        .interfaces
        .get("types")
        .expect("wasi:http should have a types interface");
    let send_informational = resolve.interfaces[*types_id]
        .functions
        .get("[method]response-outparam.send-informational")
        .expect("Should have send-informational method");
    assert!(
        matches!(send_informational.stability, Stability::Unstable { .. }),
        "response-outparam.send-informational should be unstable"
    );

    assert!(
        resolve.package_direct_deps(pkg_id).count() > 0,
        "Should have direct dependencies embedded"
    );
}

#[tokio::test]
async fn test_bad_dep_failure() {
    let (_temp, fixture_path) = common::load_fixture("wasi-http").await.unwrap();
    let lock_file = fixture_path.join("wkg.lock");
    let mut lock = LockFile::new_with_path([], &lock_file)
        .await
        .expect("Should be able to create a new lock file");
    let (_temp_cache, client) = common::get_client().await.unwrap();

    let world_file = fixture_path.join("wit").join("proxy.wit");
    let str_world = tokio::fs::read_to_string(&world_file)
        .await
        .expect("Should be able to read the world file");
    let str_world = str_world.replace(
        "import wasi:cli/stdin@0.2.3;",
        "import totally:not/real@0.2.3;",
    );
    tokio::fs::write(world_file, str_world)
        .await
        .expect("Should be able to write the world file");

    wasm_pkg_core::wit::build_package(
        &WkgConfig::default(),
        fixture_path.join("wit"),
        &mut lock,
        client,
    )
    .await
    .expect_err("Should error with a bad dependency");
}
