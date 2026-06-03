//! Integration tests for publish-time semver compatibility checking (issue #128).

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

struct Case {
    name: &'static str,
    /// Unique per row -> isolated series.
    package: &'static str,
    /// `None` skips seeding -> the candidate is the first publish in the
    /// series.
    initial: Option<(&'static str, Shape)>,
    candidate: (&'static str, Shape),
    skip_semver_check: bool,
    expected: Expected,
}

fn make_client(root: &Path) -> Client {
    let toml = format!(
        r#"
default_registry = "local"

[registry."local"]
type = "local"

[registry."local".local]
root = "{}"
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

#[tokio::test]
async fn publish_semver_check_table() {
    let tmp = TempDir::new().unwrap();
    let client = make_client(tmp.path());

    let cases = [
        Case {
            name: "first publish in empty series",
            package: "first-in-series",
            initial: None,
            candidate: ("0.1.0", Shape::Base),
            skip_semver_check: false,
            expected: Expected::Ok,
        },
        Case {
            name: "compatible in same 0.Y series",
            package: "compat-same-series",
            initial: Some(("0.1.0", Shape::Base)),
            candidate: ("0.1.1", Shape::Compatible),
            skip_semver_check: false,
            expected: Expected::Ok,
        },
        Case {
            name: "incompatible in same 0.Y series",
            package: "incompat-same-series",
            initial: Some(("0.1.0", Shape::Base)),
            candidate: ("0.1.1", Shape::Incompatible),
            skip_semver_check: false,
            expected: Expected::SemverIncompatible {
                previous: "0.1.0",
                new: "0.1.1",
            },
        },
        Case {
            name: "incompatible across 0.Y series boundary",
            package: "incompat-cross-zero-y",
            initial: Some(("0.1.0", Shape::Base)),
            candidate: ("0.2.0", Shape::Incompatible),
            skip_semver_check: false,
            expected: Expected::Ok,
        },
        Case {
            name: "incompatible across minors within a major (>=1)",
            package: "incompat-cross-minor",
            initial: Some(("1.2.0", Shape::Base)),
            candidate: ("1.3.0", Shape::Incompatible),
            skip_semver_check: false,
            expected: Expected::SemverIncompatible {
                previous: "1.2.0",
                new: "1.3.0",
            },
        },
        Case {
            name: "incompatible across major boundary",
            package: "incompat-cross-major",
            initial: Some(("1.2.0", Shape::Base)),
            candidate: ("2.0.0", Shape::Incompatible),
            skip_semver_check: false,
            expected: Expected::Ok,
        },
        Case {
            name: "incompatible with skip_semver_check",
            package: "incompat-opt-out",
            initial: Some(("0.1.0", Shape::Base)),
            candidate: ("0.1.1", Shape::Incompatible),
            skip_semver_check: true,
            expected: Expected::Ok,
        },
        Case {
            name: "duplicate version is rejected",
            package: "dup-version",
            initial: Some(("0.1.0", Shape::Base)),
            candidate: ("0.1.0", Shape::Base),
            skip_semver_check: false,
            expected: Expected::VersionAlreadyExists("0.1.0"),
        },
        Case {
            name: "duplicate version with skip_semver_check",
            package: "dup-version-opt-out",
            initial: Some(("0.1.0", Shape::Base)),
            candidate: ("0.1.0", Shape::Base),
            skip_semver_check: true,
            expected: Expected::Ok,
        },
        Case {
            // A `~0.1.*` / `^0.1` predicate excludes prereleases by design
            // (semver crate behavior), so an incompatible 0.1.1-beta.1 prior
            // must not be considered when publishing the stable 0.1.1.
            name: "prerelease priors are ignored",
            package: "ignore-prereleases",
            initial: Some(("0.1.1-beta.1", Shape::Incompatible)),
            candidate: ("0.1.1", Shape::Base),
            skip_semver_check: false,
            expected: Expected::Ok,
        },
    ];

    for case in cases {
        if let Some((init_version, init_shape)) = case.initial {
            publish(
                &client,
                wit_to_wasm(&wit_for(case.package, init_version, init_shape)),
                Default::default(),
            )
            .await
            .unwrap_or_else(|e| panic!("[{}] seeding {init_version} failed: {e:?}", case.name));
        }

        let (cand_version, cand_shape) = case.candidate;

        let opts = PublishOpts {
            skip_semver_check: case.skip_semver_check,
            ..Default::default()
        };
        let result = publish(
            &client,
            wit_to_wasm(&wit_for(case.package, cand_version, cand_shape)),
            opts,
        )
        .await;

        match (&case.expected, result) {
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
                    "[{}] previous version label mismatch",
                    case.name,
                );
                assert_eq!(
                    new.to_string(),
                    *exp_new,
                    "[{}] new version label mismatch",
                    case.name,
                );
            }
            (Expected::VersionAlreadyExists(exp), Err(Error::VersionAlreadyExists(v))) => {
                assert_eq!(
                    v.to_string(),
                    *exp,
                    "[{}] duplicate version mismatch",
                    case.name,
                );
            }
            (expected, actual) => panic!(
                "[{}] expectation mismatch\n  expected: {}\n  actual:   {:?}",
                case.name,
                describe(expected),
                actual,
            ),
        }
    }
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
