use std::cmp::Ordering;

use wasm_pkg_common::{digest::ContentDigest, package::Version};

/// Package release details.
///
/// Returned by [`crate::Client::get_release`] and passed to
/// [`crate::Client::stream_content`].
#[derive(Clone, Debug)]
pub struct Release {
    pub version: Version,
    pub content_digest: ContentDigest,
}

#[derive(Clone, Debug, Eq)]
pub struct VersionInfo {
    pub version: Version,
    pub yanked: bool,
}

impl Ord for VersionInfo {
    fn cmp(&self, other: &Self) -> Ordering {
        self.version.cmp(&other.version)
    }
}

impl PartialOrd for VersionInfo {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for VersionInfo {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
    }
}

impl std::fmt::Display for VersionInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{version}", version = self.version)
    }
}
