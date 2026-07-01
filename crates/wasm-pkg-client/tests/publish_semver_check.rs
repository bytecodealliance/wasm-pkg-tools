//! Integration tests for publish-time semver compatibility checking (issue #128).

use rstest::rstest;
use std::{io::Cursor, path::Path};
use tempfile::TempDir;
use wasm_pkg_client::{Client, Config, PublishOpts};
use wasm_pkg_common::Error;

const NAMESPACE: &str = "example";

#[derive(Clone, Copy)]
enum Shape {
    Base,
    Compatible,
    Incompatible,
}

#[derive(Clone)]
enum Expected {
    Ok,
    SemverIncompatible {
        previous: &'static str,
        new: &'static str,
    },
    VersionAlreadyExists(&'static str),
}

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

fn wit_for(package: &str, version: &str, shape: Shape) -> String {
    let world_body = match shape {
        Shape::Base => "export run: func() -> u32;",
        Shape::Compatible => "export run: func() -> u32;\n    export extra: func() -> u32;",
        Shape::Incompatible => "export run: func() -> string;",
    };
    format!(
        r#"
package {NAMESPACE}:{package}@{version};

world the-world {{
    {world_body}
}}
"#
    )
}

fn wit_to_wasm(wit: &str) -> Vec<u8> {
    let mut resolve = wit_parser::Resolve::new();
    let pkg_id = resolve
        .push_str("test.wit", wit)
        .expect("test WIT should parse");
    wit_component::encode(&resolve, pkg_id).expect("test WIT should encode")
}

async fn publish(client: &Client, bytes: Vec<u8>, opts: PublishOpts) -> Result<(), Error> {
    client
        .publish_release_data(Box::pin(Cursor::new(bytes)), opts)
        .await
        .map(|_| ())
}

fn describe(e: &Expected) -> String {
    match e {
        Expected::Ok => "Ok".into(),
        Expected::SemverIncompatible { previous, new } => {
            format!("SemverIncompatible {{ previous: {previous}, new: {new} }}")
        }
        Expected::VersionAlreadyExists(v) => format!("VersionAlreadyExists({v})"),
    }
}

#[rstest]
#[case::first_publish_in_empty_series(
    "first-in-series",
    None,
    ("0.1.0", Shape::Base),
    false,
    Expected::Ok
)]
#[case::compatible_in_same_zero_y_series(
    "compat-same-series",
    Some(("0.1.0", Shape::Base)),
    ("0.1.1", Shape::Compatible),
    false,
    Expected::Ok
)]
#[case::incompatible_in_same_zero_y_series(
    "incompat-same-series",
    Some(("0.1.0", Shape::Base)),
    ("0.1.1", Shape::Incompatible),
    false,
    Expected::SemverIncompatible { previous: "0.1.0", new: "0.1.1" }
)]
#[case::incompatible_across_zero_y_series_boundary(
    "incompat-cross-zero-y",
    Some(("0.1.0", Shape::Base)),
    ("0.2.0", Shape::Incompatible),
    false,
    Expected::Ok
)]
#[case::incompatible_across_minors_within_a_major(
    "incompat-cross-minor",
    Some(("1.2.0", Shape::Base)),
    ("1.3.0", Shape::Incompatible),
    false,
    Expected::SemverIncompatible { previous: "1.2.0", new: "1.3.0" }
)]
#[case::incompatible_across_major_boundary(
    "incompat-cross-major",
    Some(("1.2.0", Shape::Base)),
    ("2.0.0", Shape::Incompatible),
    false,
    Expected::Ok
)]
#[case::incompatible_with_skip_semver_check(
    "incompat-opt-out",
    Some(("0.1.0", Shape::Base)),
    ("0.1.1", Shape::Incompatible),
    true,
    Expected::Ok
)]
#[case::duplicate_version_is_rejected(
    "dup-version",
    Some(("0.1.0", Shape::Base)),
    ("0.1.0", Shape::Base),
    false,
    Expected::VersionAlreadyExists("0.1.0")
)]
#[case::duplicate_version_with_skip_semver_check(
    "dup-version-opt-out",
    Some(("0.1.0", Shape::Base)),
    ("0.1.0", Shape::Base),
    true,
    Expected::Ok
)]
// A `~0.1.*` / `^0.1` predicate excludes prereleases by design (semver crate
// behavior), so an incompatible 0.1.1-beta.1 prior must not be considered when
// publishing the stable 0.1.1.
#[case::prerelease_priors_are_ignored(
    "ignore-prereleases",
    Some(("0.1.1-beta.1", Shape::Incompatible)),
    ("0.1.1", Shape::Base),
    false,
    Expected::Ok
)]
#[tokio::test]
// `package` is unique per row -> isolated series. `initial` is `None` to skip
// seeding, in which case the candidate is the first publish in the series.
async fn publish_semver_check(
    #[case] package: &str,
    #[case] initial: Option<(&str, Shape)>,
    #[case] candidate: (&str, Shape),
    #[case] skip_semver_check: bool,
    #[case] expected: Expected,
) {
    let tmp = TempDir::new().unwrap();
    let client = make_client(tmp.path());

    if let Some((init_version, init_shape)) = initial {
        publish(
            &client,
            wit_to_wasm(&wit_for(package, init_version, init_shape)),
            Default::default(),
        )
        .await
        .unwrap_or_else(|e| panic!("seeding {init_version} failed: {e:?}"));
    }

    let (cand_version, cand_shape) = candidate;

    let opts = PublishOpts {
        skip_semver_check,
        ..Default::default()
    };
    let result = publish(
        &client,
        wit_to_wasm(&wit_for(package, cand_version, cand_shape)),
        opts,
    )
    .await;

    match (&expected, result) {
        (Expected::Ok, Ok(())) => {}
        (
            Expected::SemverIncompatible {
                previous: exp_prev,
                new: exp_new,
            },
            Err(Error::SemverIncompatible { previous, new, .. }),
        ) => {
            assert_eq!(
                previous.to_string(),
                *exp_prev,
                "previous version label mismatch",
            );
            assert_eq!(new.to_string(), *exp_new, "new version label mismatch");
        }
        (Expected::VersionAlreadyExists(exp), Err(Error::VersionAlreadyExists(v))) => {
            assert_eq!(v.to_string(), *exp, "duplicate version mismatch");
        }
        (expected, actual) => panic!(
            "expectation mismatch\n  expected: {}\n  actual:   {:?}",
            describe(expected),
            actual,
        ),
    }
}
