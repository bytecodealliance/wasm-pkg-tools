use async_trait::async_trait;
use futures_util::StreamExt;
use wasm_pkg_common::{
    Error,
    package::{PackageRef, Version, VersionReq},
};

use crate::{
    ContentStream,
    release::{Release, VersionInfo},
};

#[derive(Debug, Default)]
pub enum VersionSort {
    #[default]
    Ascending,
    Descending,
}

#[async_trait]
pub trait PackageLoader: Send {
    async fn list_all_versions(&self, package: &PackageRef) -> Result<Vec<VersionInfo>, Error>;

    async fn list_matching_versions(
        &self,
        package: &PackageRef,
        predicate: VersionReq,
        sort: VersionSort,
    ) -> Result<Vec<VersionInfo>, Error> {
        let versions = match self.list_all_versions(package).await {
            Ok(v) => v,
            Err(Error::PackageNotFound) => Vec::new(),
            Err(e) => return Err(e),
        };

        let mut matching: Vec<VersionInfo> = versions
            .into_iter()
            .filter(|v| predicate.matches(&v.version))
            .collect();

        match sort {
            VersionSort::Ascending => matching.sort_by(|a, b| a.version.cmp(&b.version)),
            VersionSort::Descending => matching.sort_by(|a, b| b.version.cmp(&a.version)),
        };

        Ok(matching)
    }

    async fn get_release(&self, package: &PackageRef, version: &Version) -> Result<Release, Error>;

    async fn stream_content_unvalidated(
        &self,
        package: &PackageRef,
        release: &Release,
    ) -> Result<ContentStream, Error>;

    async fn stream_content(
        &self,
        package: &PackageRef,
        release: &Release,
    ) -> Result<ContentStream, Error> {
        let stream = self.stream_content_unvalidated(package, release).await?;
        Ok(release.content_digest.validating_stream(stream).boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::{ContentStream, PackageLoader, Release, VersionInfo, VersionSort};
    use async_trait::async_trait;
    use rstest::rstest;
    use wasm_pkg_common::{
        package::{PackageRef, Version, VersionReq},
        Error,
    };

    #[derive(Clone, Debug)]
    struct VerifiablePackageLoader {
        history: Vec<VersionInfo>,
    }

    impl VerifiablePackageLoader {
        fn new(history: &[Version]) -> Self {
            Self {
                history: history
                    .iter()
                    .cloned()
                    .map(|version| VersionInfo {
                        version,
                        yanked: false,
                    })
                    .collect(),
            }
        }
    }

    #[async_trait]
    impl PackageLoader for VerifiablePackageLoader {
        async fn list_all_versions(
            &self,
            _package: &PackageRef,
        ) -> Result<Vec<VersionInfo>, Error> {
            Ok(self.history.clone())
        }

        async fn get_release(
            &self,
            _package: &PackageRef,
            _version: &Version,
        ) -> Result<Release, Error> {
            panic!("get_release is not needed in this unit test")
        }

        async fn stream_content_unvalidated(
            &self,
            _package: &PackageRef,
            _release: &Release,
        ) -> Result<ContentStream, Error> {
            panic!("stream_content_unvalidated is not needed in this unit test")
        }
    }

    fn v(input: &str) -> Version {
        input.parse().expect("valid semver in test case")
    }

    fn versions(inputs: &[&str]) -> Vec<Version> {
        inputs.iter().map(|s| v(s)).collect()
    }

    // These cases include the examples from the function docs and edge cases
    // for lane filtering behavior.
    #[rstest]
    #[case::target_0_0_0(
        "~0.0.*",
        VersionSort::Ascending,
        &["0.0.0", "0.0.1", "0.1.0", "1.0.0"],
        &["0.0.0", "0.0.1"]
    )]
    #[case::target_0_0_3(
        "~0.0.*",
        VersionSort::Ascending,
        &["0.0.0", "0.0.3", "0.0.7", "0.1.0"],
        &["0.0.0", "0.0.3", "0.0.7"]
    )]
    #[case::target_1_0_0(
        "~1.0.*",
        VersionSort::Ascending,
        &["1.0.0", "1.0.9", "1.1.0", "2.0.0"],
        &["1.0.0", "1.0.9"]
    )]
    #[case::target_2_2_0(
        "~2.2.*",
        VersionSort::Ascending,
        &["2.1.9", "2.2.0", "2.2.5", "2.3.0"],
        &["2.2.0", "2.2.5"]
    )]
    #[case::empty_history("~1.2.*", VersionSort::Ascending, &[], &[])]
    #[case::no_matching_major_minor_in_history(
        "~3.4.*",
        VersionSort::Ascending,
        &["3.5.0", "3.6.1", "4.4.5"],
        &[]
    )]
    #[case::all_patches_in_series(
        "~1.2.*",
        VersionSort::Ascending,
        &["1.2.0", "1.2.1", "1.2.99", "1.3.0", "0.2.9"],
        &["1.2.0", "1.2.1", "1.2.99"]
    )]
    // The exclusion of pre-release versions in range queries is a bit
    // unintuitive but apparently intentional.
    //
    // https://github.com/dtolnay/semver/issues/98
    #[case::pre_release_excluded_from_series(
        "~1.2.*",
        VersionSort::Ascending,
        &["1.2.0", "1.2.1-beta.2", "1.2.4+build.7", "1.3.0-alpha.1"],
        &["1.2.0", "1.2.4+build.7"]
    )]
    #[case::duplication_is_preserved(
        "~1.2.*",
        VersionSort::Ascending,
        &["1.2.1", "1.2.1", "1.2.2", "1.3.0"],
        &["1.2.1", "1.2.1", "1.2.2"]
    )]
    #[case::descending_sort_orders_matches_high_to_low(
        "~1.2.*",
        VersionSort::Descending,
        &["1.2.0", "1.2.5", "1.2.1", "1.3.0"],
        &["1.2.5", "1.2.1", "1.2.0"]
    )]
    #[case::descending_sort_with_empty_matches(
        "~9.9.*",
        VersionSort::Descending,
        &["1.0.0", "2.0.0"],
        &[]
    )]
    #[tokio::test]
    async fn list_matching_versions_filters_by_version_req(
        #[case] req: &str,
        #[case] sort: VersionSort,
        #[case] history: &[&str],
        #[case] expected: &[&str],
    ) {
        let history = versions(history);
        let expected = versions(expected);
        let filter = VersionReq::parse(req).expect("valid series req");
        let package: PackageRef = "example:package".parse().expect("valid package ref");

        let loader = VerifiablePackageLoader::new(&history);

        let got: Vec<Version> = loader
            .list_matching_versions(&package, filter, sort)
            .await
            .expect("list_matching_versions should succeed")
            .into_iter()
            .map(|v| v.version)
            .collect();

        assert_eq!(got, expected.as_slice());
    }
}
