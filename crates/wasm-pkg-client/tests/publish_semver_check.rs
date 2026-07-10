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

// ---------------------------------------------------------------------------
// Multi-world packages (issue: semver_check assumed at most one world).
//
// A WIT package may declare more than one world. Semver checking must compare
// worlds pairwise by name: shared worlds must stay compatible, adding a world
// is additive (OK), and removing a world within the same compat range is
// breaking (strict policy).
// ---------------------------------------------------------------------------

/// Which worlds a multi-world fixture declares, and the body of each.
struct MultiWorld {
    /// `alpha` world body, if present.
    alpha: Option<WorldDiff>,
    /// `beta` world body, if present.
    beta: Option<WorldDiff>,
}

fn multiworld_wit(package: &str, version: &str, worlds: &MultiWorld) -> String {
    let mut out = format!("\npackage {NAMESPACE}:{package}@{version};\n");
    if let Some(diff) = worlds.alpha {
        out.push_str(&format!("\nworld alpha {{\n    {diff}\n}}\n"));
    }
    if let Some(diff) = worlds.beta {
        out.push_str(&format!("\nworld beta {{\n    {diff}\n}}\n"));
    }
    out
}

#[rstest]
// Both worlds compatible across a minor bump within a major -> OK.
#[case::multiworld_all_compatible(
    "mw-all-compat",
    ("1.2.0", MultiWorld { alpha: Some(WorldDiff::AddBase), beta: Some(WorldDiff::AddBase) }),
    ("1.3.0", MultiWorld { alpha: Some(WorldDiff::AddExtra), beta: Some(WorldDiff::AddExtra) }),
    Ok(())
)]
// One shared world (beta) breaks across a minor bump -> incompatible.
#[case::multiworld_one_incompatible(
    "mw-one-incompat",
    ("1.2.0", MultiWorld { alpha: Some(WorldDiff::AddBase), beta: Some(WorldDiff::AddBase) }),
    ("1.3.0", MultiWorld { alpha: Some(WorldDiff::AddExtra), beta: Some(WorldDiff::ChangeBase) }),
    Err(semver_incompatible("1.2.0", "1.3.0"))
)]
// Adding a world across a minor bump is additive -> OK.
#[case::multiworld_added_world(
    "mw-added",
    ("1.2.0", MultiWorld { alpha: Some(WorldDiff::AddBase), beta: None }),
    ("1.3.0", MultiWorld { alpha: Some(WorldDiff::AddExtra), beta: Some(WorldDiff::AddBase) }),
    Ok(())
)]
// Removing a world within the same major is breaking (strict policy) -> incompatible.
#[case::multiworld_removed_world_same_major(
    "mw-removed",
    ("1.2.0", MultiWorld { alpha: Some(WorldDiff::AddBase), beta: Some(WorldDiff::AddBase) }),
    ("1.3.0", MultiWorld { alpha: Some(WorldDiff::AddExtra), beta: None }),
    Err(semver_incompatible("1.2.0", "1.3.0"))
)]
// Removing a world across a major boundary is allowed: the versions fall in
// different compatibility ranges, so `semver_check` never compares them.
#[case::multiworld_removed_world_across_major(
    "mw-removed-cross-major",
    ("1.2.0", MultiWorld { alpha: Some(WorldDiff::AddBase), beta: Some(WorldDiff::AddBase) }),
    ("2.0.0", MultiWorld { alpha: Some(WorldDiff::AddBase), beta: None }),
    Ok(())
)]
#[tokio::test]
async fn publish_semver_check_multiworld(
    #[case] package: &str,
    #[case] initial: (&str, MultiWorld),
    #[case] candidate: (&str, MultiWorld),
    #[case] expected: Result<(), Error>,
) {
    let tmp = TempDir::new().unwrap();
    let client = make_client(tmp.path());

    let (init_version, init_worlds) = initial;
    publish(
        &client,
        wit_to_wasm(&multiworld_wit(package, init_version, &init_worlds)),
        Default::default(),
    )
    .await
    .unwrap_or_else(|e| panic!("seeding {init_version} failed: {e:?}"));

    let (cand_version, cand_worlds) = candidate;
    let result = publish(
        &client,
        wit_to_wasm(&multiworld_wit(package, cand_version, &cand_worlds)),
        Default::default(),
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
        _ => panic!("expectation mismatch\n  expected: {expected:?}\n  actual:   {result:?}",),
    }
}
