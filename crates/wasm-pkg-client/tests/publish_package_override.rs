use std::{io::Cursor, path::Path};
use tempfile::TempDir;
use wasm_pkg_client::{Client, Config, PublishOpts};

const WIT: &str = r#"
package example:embedded@0.1.0;

world the-world {
    export base: func() -> u32;
}
"#;

fn make_client(root: &Path) -> Client {
    let toml = format!(
        r#"
default_registry = "local"

[registry."local"]
type = "local"

[registry."local".local]
root = '{}'
"#,
        root.display(),
    );
    let config = Config::from_toml(&toml).expect("local-backend config should parse");
    Client::new(config)
}

fn component_bytes() -> Vec<u8> {
    let mut resolve = wit_parser::Resolve::new();
    let pkg = resolve
        .push_str("test.wit", WIT)
        .expect("test WIT should parse");
    let world = resolve
        .select_world(&[pkg], None)
        .expect("test WIT should have exactly one world");
    let mut module =
        wit_component::dummy_module(&resolve, world, wit_parser::ManglingAndAbi::Standard32);
    wit_component::embed_component_metadata(
        &mut module,
        &resolve,
        world,
        wit_component::StringEncoding::UTF8,
    )
    .expect("component metadata should embed");
    wit_component::ComponentEncoder::default()
        .module(&module)
        .expect("dummy module should be accepted")
        .validate(true)
        .encode()
        .expect("dummy module should encode as a component")
}

#[tokio::test]
async fn override_supplies_identity_for_component() {
    let tmp = TempDir::new().unwrap();
    let client = make_client(tmp.path());

    let opts = PublishOpts {
        package: Some(("example:app".parse().unwrap(), "1.0.0".parse().unwrap())),
        ..Default::default()
    };
    let (package, version) = client
        .publish_release_data(Box::pin(Cursor::new(component_bytes())), opts)
        .await
        .expect("publishing a component with an explicit identity should succeed");

    assert_eq!(package, "example:app".parse().unwrap());
    assert_eq!(version, "1.0.0".parse().unwrap());
    client
        .get_release(&package, &version)
        .await
        .expect("published release should be retrievable at the supplied coordinate");
}
