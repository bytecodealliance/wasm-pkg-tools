use crate::{
    release::{Release, VersionInfo},
    ContentStream,
};
use async_trait::async_trait;
use futures_util::StreamExt;
use wasm_pkg_common::{
    package::{PackageRef, Version, VersionReq},
    Error,
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
        let mut versions = match self.list_all_versions(package).await {
            Ok(v) => v,
            Err(Error::PackageNotFound) => Vec::new(),
            Err(e) => return Err(e),
        };

        match sort {
            VersionSort::Ascending => versions.sort_by(|a, b| a.version.cmp(&b.version)),
            VersionSort::Descending => versions.sort_by(|a, b| b.version.cmp(&a.version)),
        };

        let matching: Vec<VersionInfo> = versions
            .into_iter()
            .filter(|v| predicate.matches(&v.version))
            .collect();

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

    #[derive(Debug)]
    struct Case {
        name: &'static str,
        req: &'static str,
        sort: VersionSort,
        history: &'static [&'static str],
        expected: &'static [&'static str],
    }

    fn v(input: &str) -> Version {
        input.parse().expect("valid semver in test case")
    }

    fn versions(inputs: &[&str]) -> Vec<Version> {
        inputs.iter().map(|s| v(s)).collect()
    }

    /// `list_matching_versions` is a generic `VersionReq` filter; this test
    /// exercises it with the cargo-`^` shaped series masks that the publish
    /// gate constructs in `fetch_semver_series` (see `lib.rs`):
    ///
    /// - `X.y.z` (X >= 1) -> `X.*`
    /// - `0.Y.z` (Y >= 1) -> `0.Y.*`
    /// - `0.0.Z`          -> exact `0.0.Z`
    #[tokio::test]
    async fn list_matching_versions_filters_by_version_req_table_driven() {
        // These cases include the examples from the function docs and edge
        // cases for lane filtering behavior.
        let cases = [
            Case {
                name: "target 0.0.0 -> 0.0.*",
                req: "~0.0.*",
                sort: VersionSort::Ascending,
                history: &["0.0.0", "0.0.1", "0.1.0", "1.0.0"],
                expected: &["0.0.0", "0.0.1"],
            },
            Case {
                name: "target 0.0.3 -> 0.0.*",
                req: "~0.0.*",
                sort: VersionSort::Ascending,
                history: &["0.0.0", "0.0.3", "0.0.7", "0.1.0"],
                expected: &["0.0.0", "0.0.3", "0.0.7"],
            },
            Case {
                name: "target 1.0.0 -> 1.0.*",
                req: "~1.0.*",
                sort: VersionSort::Ascending,
                history: &["1.0.0", "1.0.9", "1.1.0", "2.0.0"],
                expected: &["1.0.0", "1.0.9"],
            },
            Case {
                name: "target 2.2.0 -> 2.2.*",
                req: "~2.2.*",
                sort: VersionSort::Ascending,
                history: &["2.1.9", "2.2.0", "2.2.5", "2.3.0"],
                expected: &["2.2.0", "2.2.5"],
            },
            Case {
                name: "empty history",
                req: "~1.2.*",
                sort: VersionSort::Ascending,
                history: &[],
                expected: &[],
            },
            Case {
                name: "no matching major minor in history",
                req: "~3.4.*",
                sort: VersionSort::Ascending,
                history: &["3.5.0", "3.6.1", "4.4.5"],
                expected: &[],
            },
            Case {
                name: "all patches in series",
                req: "~1.2.*",
                sort: VersionSort::Ascending,
                history: &["1.2.0", "1.2.1", "1.2.99", "1.3.0", "0.2.9"],
                expected: &["1.2.0", "1.2.1", "1.2.99"],
            },
            Case {
                // The exclusion of pre-release versions in range queries
                // is a bit unintuitive but apparently intentional.
                //
                // https://github.com/dtolnay/semver/issues/98
                name: "pre-release excluded from series",
                req: "~1.2.*",
                sort: VersionSort::Ascending,
                history: &["1.2.0", "1.2.1-beta.2", "1.2.4+build.7", "1.3.0-alpha.1"],
                expected: &["1.2.0", "1.2.4+build.7"],
            },
            Case {
                name: "duplication is preserved",
                req: "~1.2.*",
                sort: VersionSort::Ascending,
                history: &["1.2.1", "1.2.1", "1.2.2", "1.3.0"],
                expected: &["1.2.1", "1.2.1", "1.2.2"],
            },
            Case {
                name: "descending sort orders matches high to low",
                req: "~1.2.*",
                sort: VersionSort::Descending,
                history: &["1.2.0", "1.2.5", "1.2.1", "1.3.0"],
                expected: &["1.2.5", "1.2.1", "1.2.0"],
            },
            Case {
                name: "descending sort with empty matches",
                req: "~9.9.*",
                sort: VersionSort::Descending,
                history: &["1.0.0", "2.0.0"],
                expected: &[],
            },
        ];

        for case in cases {
            let history = versions(case.history);
            let expected = versions(case.expected);
            let filter = VersionReq::parse(case.req).expect("valid series req");
            let package: PackageRef = "example:package".parse().expect("valid package ref");

            let loader = VerifiablePackageLoader::new(&history);

            let got: Vec<Version> = loader
                .list_matching_versions(&package, filter, case.sort)
                .await
                .expect("list_matching_versions should succeed")
                .into_iter()
                .map(|v| v.version)
                .collect();

            assert_eq!(got, expected.as_slice(), "case failed: {}", case.name);
        }
    }
}
