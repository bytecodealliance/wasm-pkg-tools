use wasm_pkg_core::{config::Config as WkgConfig, lock::LockFile};
use wit_component::DecodedWasm;

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
        "0.2.0",
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

    let name = resolve
        .package_names
        .iter()
        .find_map(|(name, id)| (pkg_id == *id).then_some(name))
        .expect("Should be able to find the package name");

    assert_eq!(
        name.to_string(),
        "wasi:http@0.2.0",
        "Should have the correct package name"
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
        "import wasi:cli/stdin@0.2.0;",
        "import totally:not/real@0.2.0;",
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
