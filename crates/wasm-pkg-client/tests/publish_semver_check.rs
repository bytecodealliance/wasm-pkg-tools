//! Integration tests for publish-time semver compatibility checking (issue #128).

use rstest::rstest;
use std::{fmt, io::Cursor, path::Path};
use tempfile::TempDir;
use wasm_pkg_client::{Client, Config, PublishOpts};
use wasm_pkg_common::Error;

const NAMESPACE: &str = "example";

#[derive(Clone, Copy)]
enum WorldDiff {
    // + export base: func() -> u32;
    AddBase,
    // + export extra: func() -> u32;
    AddExtra,
    // - export base: func() -> u32;
    // + export base: func() -> string;
    ChangeBase,
}

impl fmt::Display for WorldDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let body = match self {
            WorldDiff::AddBase => "export base: func() -> u32;",
            WorldDiff::AddExtra => "export base: func() -> u32;\n    export extra: func() -> u32;",
            WorldDiff::ChangeBase => "export base: func() -> string;",
        };
        f.write_str(body)
    }
}

fn semver_incompatible(previous: &str, new: &str) -> Error {
    Error::SemverIncompatible {
        previous: previous.parse().unwrap(),
        new: new.parse().unwrap(),
        source: anyhow::anyhow!(""),
    }
}

fn version_already_exists(version: &str) -> Error {
    Error::VersionAlreadyExists(version.parse().unwrap())
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

fn wit_for(package: &str, version: &str, diff: WorldDiff) -> String {
    format!(
        r#"
package {NAMESPACE}:{package}@{version};

world the-world {{
    {diff}
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

#[rstest]
#[case::first_publish_in_empty_series(
    "first-in-series",
    None,
    ("0.1.0", WorldDiff::AddBase),
    false,
    Ok(())
)]
#[case::compatible_in_same_zero_y_series(
    "compat-same-series",
    Some(("0.1.0", WorldDiff::AddBase)),
    ("0.1.1", WorldDiff::AddExtra),
    false,
    Ok(())
)]
#[case::incompatible_in_same_zero_y_series(
    "incompat-same-series",
    Some(("0.1.0", WorldDiff::AddBase)),
    ("0.1.1", WorldDiff::ChangeBase),
    false,
    Err(semver_incompatible("0.1.0", "0.1.1"))
)]
#[case::incompatible_across_zero_y_series_boundary(
    "incompat-cross-zero-y",
    Some(("0.1.0", WorldDiff::AddBase)),
    ("0.2.0", WorldDiff::ChangeBase),
    false,
    Ok(())
)]
#[case::incompatible_across_minors_within_a_major(
    "incompat-cross-minor",
    Some(("1.2.0", WorldDiff::AddBase)),
    ("1.3.0", WorldDiff::ChangeBase),
    false,
    Err(semver_incompatible("1.2.0", "1.3.0"))
)]
#[case::incompatible_across_major_boundary(
    "incompat-cross-major",
    Some(("1.2.0", WorldDiff::AddBase)),
    ("2.0.0", WorldDiff::ChangeBase),
    false,
    Ok(())
)]
#[case::incompatible_with_skip_semver_check(
    "incompat-opt-out",
    Some(("0.1.0", WorldDiff::AddBase)),
    ("0.1.1", WorldDiff::ChangeBase),
    true,
    Ok(())
)]
#[case::duplicate_version_is_rejected(
    "dup-version",
    Some(("0.1.0", WorldDiff::AddBase)),
    ("0.1.0", WorldDiff::AddBase),
    false,
    Err(version_already_exists("0.1.0"))
)]
#[case::duplicate_version_with_skip_semver_check(
    "dup-version-opt-out",
    Some(("0.1.0", WorldDiff::AddBase)),
    ("0.1.0", WorldDiff::AddBase),
    true,
    Ok(())
)]
// A `~0.1.*` / `^0.1` predicate excludes prereleases by design (semver crate
// behavior), so an incompatible 0.1.1-beta.1 prior must not be considered when
// publishing the stable 0.1.1.
#[case::prerelease_priors_are_ignored(
    "ignore-prereleases",
    Some(("0.1.1-beta.1", WorldDiff::ChangeBase)),
    ("0.1.1", WorldDiff::AddBase),
    false,
    Ok(())
)]
#[tokio::test]
// `package` is unique per row -> isolated series. `initial` is `None` to skip
// seeding, in which case the candidate is the first publish in the series.
async fn publish_semver_check(
    #[case] package: &str,
    #[case] initial: Option<(&str, WorldDiff)>,
    #[case] candidate: (&str, WorldDiff),
    #[case] skip_semver_check: bool,
    #[case] expected: Result<(), Error>,
) {
    let tmp = TempDir::new().unwrap();
    let client = make_client(tmp.path());

    if let Some((init_version, init_diff)) = initial {
        publish(
            &client,
            wit_to_wasm(&wit_for(package, init_version, init_diff)),
            Default::default(),
        )
        .await
        .unwrap_or_else(|e| panic!("seeding {init_version} failed: {e:?}"));
    }

    let (cand_version, cand_diff) = candidate;

    let opts = PublishOpts {
        skip_semver_check,
        ..Default::default()
    };
    let result = publish(
        &client,
        wit_to_wasm(&wit_for(package, cand_version, cand_diff)),
        opts,
    )
    .await;

    match (&expected, &result) {
        (Ok(()), Ok(())) => {}
        (
            Err(Error::SemverIncompatible {
                previous: exp_prev,
                new: exp_new,
                ..
            }),
            Err(Error::SemverIncompatible { previous, new, .. }),
        ) => {
            assert_eq!(previous, exp_prev, "previous version mismatch");
            assert_eq!(new, exp_new, "new version mismatch");
        }
        (Err(Error::VersionAlreadyExists(exp)), Err(Error::VersionAlreadyExists(actual))) => {
            assert_eq!(actual, exp, "duplicate version mismatch");
        }
        _ => panic!("expectation mismatch\n  expected: {expected:?}\n  actual:   {result:?}",),
    }
}
